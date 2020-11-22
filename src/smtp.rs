use core::future::Future;

use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
};

use crate::utils::ConnectionInfo;
use crate::Result;

const MSG_250_OK: &[u8] = b"250 OK\r\n";
const MSG_354_NEXT_DATA: &[u8] = b"354 Start mail input; end with <CRLF>.<CRLF>\r\n";
const MSG_500_LENGTH_TOO_LONG: &[u8] = b"500 Line too long.\r\n";
const MSG_502_NOT_IMPLEMENTED: &[u8] = b"502 Command not implemented\r\n";
const MSG_503_BAD_SEQUENCE: &[u8] = b"503 Bad sequence of commands\r\n";

pub async fn serve_smtp(port: u16, server_name: String) -> Result<()>
where
{
    let addr = format!("localhost:{}", port)
        .to_socket_addrs()
        .await?
        .next()
        .unwrap();
    trace!("Binding on {}", addr);

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Unable to bind {}: {}", addr, e))?;

    let mut incoming = listener.incoming();
    info!("Listening on {:?}", listener.local_addr());

    while let Some(stream) = incoming.next().await {
        let conn = if let Ok(ref stream) = stream {
            ConnectionInfo::new(stream.local_addr().ok(), stream.peer_addr().ok())
        } else {
            ConnectionInfo::default()
        };
        //let stream = stream.map_err(|e| e.into())?;
        let stream = stream?;
        info!("Accepting new connection from: {}", stream.peer_addr()?);
        spawn_task_and_swallow_log_errors(
            format!("TCP transmission {}", conn),
            smtp_handle(stream, conn, server_name.clone()),
        );
    }
    Ok(())
}

fn spawn_task_and_swallow_log_errors<F>(task_name: String, fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    task::spawn(async move { log_errors(task_name, fut).await.unwrap_or_default() })
}

async fn log_errors<F, T, E>(task_name: String, fut: F) -> Option<T>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
{
    match fut.await {
        Ok(r) => {
            info!("{} completes successfully.", task_name);
            Some(r)
        }
        Err(e) => {
            error!("Error in {}: {}", task_name, e);
            None
        }
    }
}

#[derive(Debug)]
enum Command {
    Hello(String),
    Ehllo(String),
    Mail(String),
    Recipient(String),
    Data(String),
    DataStart,
    DataEnd,
    Noop,
    Reset,
    Quit,
    Error(String),
}

#[derive(Clone)]
struct Smtp {
    server_name: String,
    write_stream: TcpStream,
    remote_name: Option<String>,
    addr_from: Option<String>,
    addr_to: Vec<String>,
    receive_data: bool,
    data: String,
}

impl Smtp {
    pub fn new(write_stream: &TcpStream) -> Self {
        Self {
            server_name: "".to_string(),
            write_stream: write_stream.clone(),
            remote_name: None,
            addr_from: None,
            addr_to: vec![],
            receive_data: false,
            data: "".to_string(),
        }
    }

    async fn write(&mut self, messsage: &[u8]) -> Result<()> {
        self.write_stream.write_all(messsage).await?;
        Ok(())
    }

    pub async fn set_server_name(&mut self, server_name: String) -> Result<()> {
        // Write greeting server string
        self.server_name = server_name;
        self.write(format!("220 {} ESMTP\r\n", self.server_name).as_bytes())
            .await?;
        Ok(())
    }

    pub fn process_line(&self, line: String) -> Command {
        // debug!("texte: {}", line);
        if !self.receive_data {
            match line.to_uppercase().as_str() {
                "DATA" => Command::DataStart,
                "RSET" => Command::Reset,
                "QUIT" => Command::Quit,
                line if line.len() > 5 && &line[..5] == "HELO " => {
                    Command::Hello(line[5..].trim().to_string())
                }
                line if line.len() > 5 && &line[..5] == "EHLO " => {
                    Command::Ehllo(line[5..].trim().to_string())
                }
                line if line.len() > 10 && &line[..10] == "MAIL FROM:" => {
                    Command::Mail(line[10..].trim().to_string())
                }
                line if line.len() > 8 && &line[..8] == "RCPT TO:" => {
                    Command::Recipient(line[8..].trim().to_string())
                }
                line if (line.len() == 4 && line == "NOOP")
                    || (line.len() > 4 && &line[..5] == "NOOP ") =>
                {
                    Command::Noop
                }
                _ => Command::Error(line),
            }
        } else if &line == "." {
            Command::DataEnd
        } else {
            Command::Data(line)
        }
    }

    pub fn reset(&mut self) {
        self.data.clear();
        self.receive_data = false;
        self.addr_to.clear();
        self.remote_name = None;
        self.addr_from = None;
    }

    fn push_data(&mut self, line: String) {
        let idx = if !line.is_empty() && &line[0..1] == "." {
            1
        } else {
            0
        };
        if !self.data.is_empty() {
            self.data.push_str("\r\n");
        }
        self.data.push_str(&line[idx..]);
    }

    pub fn is_valid(&self, action: &Command) -> bool {
        match action {
            Command::Hello(_) | Command::Ehllo(_) => {
                self.addr_from.is_none() && self.addr_to.is_empty()
            }
            Command::Mail(_) => self.remote_name.is_some() && self.addr_to.is_empty(),
            Command::Recipient(_) => self.remote_name.is_some() && self.addr_from.is_some(),
            Command::Data(_) => {
                self.remote_name.is_some() && self.addr_from.is_some() && !self.addr_to.is_empty()
            }
            Command::Noop | Command::Quit | Command::Reset => true,
            Command::DataStart | Command::DataEnd | Command::Error(_) => true,
        }
    }

    pub async fn process_command(&mut self, command: Command) -> Result<bool> {
        Ok(match command {
            action if !self.is_valid(&action) => {
                self.write(MSG_503_BAD_SEQUENCE).await?;
                true
            }
            Command::Noop => {
                self.write(MSG_250_OK).await?;
                true
            }
            Command::Ehllo(remote_name) | Command::Hello(remote_name) => {
                self.remote_name = Some(remote_name.clone());
                self.write(format!("250 {}\r\n", self.server_name).as_bytes())
                    .await?;
                true
            }
            Command::Reset => {
                self.reset();
                self.write(MSG_250_OK).await?;
                true
            }
            Command::Mail(from) => {
                if from.len() > 64 {
                    error!("Username too long.");
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                } else {
                    self.addr_from = Some(from);
                    self.write(MSG_250_OK).await?
                }
                true
            }
            Command::Recipient(to) => {
                self.addr_to.push(to);
                self.write(MSG_250_OK).await?;
                true
            }
            Command::DataStart => {
                self.receive_data = true;
                self.write(MSG_354_NEXT_DATA).await?;
                true
            }
            Command::Data(line) => {
                if line.len() > 1001 {
                    error!(
                        "Data line length ({}) cannot exceed 1000 characters.",
                        line.len()
                    );
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                }
                self.push_data(line);
                true
            }
            Command::DataEnd => {
                // TODO:
                warn!("Need to implement storage!!!");
                //smtp.save();
                trace!("{}", self.data);

                self.receive_data = false;
                self.addr_from = None;
                self.addr_to.clear();
                self.data.clear();

                self.write(MSG_250_OK).await?;
                true
            }
            Command::Quit => {
                self.write(
                    format!(
                        "221 {} Service closing transmission channel\r\n",
                        self.server_name
                    )
                    .as_bytes(),
                )
                .await?;

                false
            }
            Command::Error(err) => {
                error!("Unsupported command: \"{}\"", err);

                self.write(MSG_502_NOT_IMPLEMENTED).await?;

                true
            }
        })
    }
}

async fn smtp_handle(stream: TcpStream, conn: ConnectionInfo, server_name: String) -> Result<()> {
    let mut smtp = Smtp::new(&stream);

    smtp.set_server_name(server_name).await?;

    let reader = BufReader::new(&stream);
    let mut lines = reader.lines();

    // Begin command loop
    while let Some(line) = lines.next().await {
        let action = smtp.process_line(line?);
        debug!("{:?}", action);
        let dont_quit = smtp.process_command(action).await?;
        if !dont_quit {
            break;
        }
    }

    info!(">>> {}", conn);

    Ok(())
}
