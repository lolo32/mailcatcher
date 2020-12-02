use async_channel::Sender;
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
};

use crate::{mail::Mail, utils::*, Result};

const MSG_250_OK: &[u8] = b"250 OK\r\n";
const MSG_354_NEXT_DATA: &[u8] = b"354 Start mail input; end with <CRLF>.<CRLF>\r\n";
const MSG_500_LENGTH_TOO_LONG: &[u8] = b"500 Line too long.\r\n";
const MSG_502_NOT_IMPLEMENTED: &[u8] = b"502 Command not implemented\r\n";
const MSG_503_BAD_SEQUENCE: &[u8] = b"503 Bad sequence of commands\r\n";

pub async fn serve_smtp(
    port: u16,
    server_name: String,
    mails: Sender<Mail>,
    use_starttls: bool,
) -> Result<()> {
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
    info!("SMTP listening on {:?}", listener.local_addr());

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
            smtp_handle(
                stream,
                conn,
                server_name.clone(),
                use_starttls,
                mails.clone(),
            ),
        );
    }
    Ok(())
}

#[derive(Debug)]
enum Command {
    Hello(String),
    Ehllo(String),
    StartTls,
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
    use_starttls: bool,
    remote_name: Option<String>,
    addr_from: Option<String>,
    addr_to: Vec<String>,
    receive_data: bool,
    data: String,
}

impl Smtp {
    pub fn new(write_stream: &TcpStream, use_starttls: bool) -> Self {
        Self {
            server_name: "".to_string(),
            write_stream: write_stream.clone(),
            use_starttls,
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
            match line.to_lowercase().as_str() {
                "data" => Command::DataStart,
                "rset" => Command::Reset,
                "quit" => Command::Quit,
                "starttls" if self.use_starttls => Command::StartTls,
                line if line.len() > 5 && &line[..5] == "helo " => {
                    Command::Hello(line[5..].trim().to_string())
                }
                line if line.len() > 5 && &line[..5] == "ehlo " => {
                    Command::Ehllo(line[5..].trim().to_string())
                }
                line if line.len() > 10 && &line[..10] == "mail from:" => {
                    Command::Mail(line[10..].trim().to_string())
                }
                line if line.len() > 8 && &line[..8] == "rcpt to:" => {
                    Command::Recipient(line[8..].trim().to_string())
                }
                line if (line.len() == 4 && line == "noop")
                    || (line.len() > 4 && &line[..5] == "noop ") =>
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
            Command::StartTls if self.use_starttls => true,
            Command::StartTls => false,
        }
    }

    pub async fn process_command(&mut self, command: Command) -> Result<(bool, Option<Mail>)> {
        Ok(match command {
            action if !self.is_valid(&action) => {
                self.write(MSG_503_BAD_SEQUENCE).await?;
                (true, None)
            }
            Command::Noop => {
                self.write(MSG_250_OK).await?;
                (true, None)
            }
            Command::Ehllo(remote_name) | Command::Hello(remote_name) => {
                self.remote_name = Some(remote_name.clone());
                let greeting = if self.use_starttls {
                    format!("250-{}\r\n250 STARTTLS\r\n", self.server_name)
                } else {
                    format!("250 {}\r\n", self.server_name)
                };
                self.write(greeting.as_bytes()).await?;
                (true, None)
            }
            Command::StartTls => {
                // TODO: need to implement it
                unimplemented!();
                (true, None)
            }
            Command::Reset => {
                self.reset();
                self.write(MSG_250_OK).await?;
                (true, None)
            }
            Command::Mail(from) => {
                if from.len() > 64 {
                    error!("Username too long.");
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                } else {
                    self.addr_from = Some(from);
                    self.write(MSG_250_OK).await?
                }
                (true, None)
            }
            Command::Recipient(to) => {
                self.addr_to.push(to);
                self.write(MSG_250_OK).await?;
                (true, None)
            }
            Command::DataStart => {
                self.receive_data = true;
                self.write(MSG_354_NEXT_DATA).await?;
                (true, None)
            }
            Command::Data(line) => {
                if line.len() > 1000 {
                    error!(
                        "Data line length ({}) cannot exceed 998 characters.",
                        line.len()
                    );
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                }
                self.push_data(line);
                (true, None)
            }
            Command::DataEnd => {
                // TODO:
                warn!("Need to implement storage!!!");
                //smtp.save();
                trace!("{}", self.data);
                let mail = Mail::new(self.addr_from.as_ref().unwrap(), &self.addr_to, &self.data);

                self.receive_data = false;
                self.addr_from = None;
                self.addr_to.clear();
                self.data.clear();

                self.write(MSG_250_OK).await?;
                (true, Some(mail))
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
                (false, None)
            }
            Command::Error(err) => {
                error!("Unsupported command: \"{}\"", err);

                self.write(MSG_502_NOT_IMPLEMENTED).await?;
                (true, None)
            }
        })
    }
}

async fn smtp_handle(
    stream: TcpStream,
    conn: ConnectionInfo,
    server_name: String,
    use_starttls: bool,
    mails: Sender<Mail>,
) -> Result<()> {
    let mut smtp = Smtp::new(&stream, use_starttls);

    smtp.set_server_name(server_name).await?;

    let reader = BufReader::new(&stream);
    let mut lines = reader.lines();

    // Begin command loop
    while let Some(line) = lines.next().await {
        let action = smtp.process_line(line?);
        trace!("{:?}", action);
        let (dont_quit, mail) = smtp.process_command(action).await?;
        if let Some(mail) = mail {
            mails.send(mail).await?;
        };
        if !dont_quit {
            break;
        }
    }

    info!(">>> {}", conn);

    Ok(())
}
