use std::borrow::Cow;

use async_channel::Sender;
use async_std::{
    io::BufReader,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
};
use futures::{
    stream::FuturesUnordered,
    {future, AsyncBufReadExt, AsyncWriteExt, StreamExt},
};

use crate::{mail::Mail, utils::*};
use command::Command;

mod command;

const MSG_250_OK: &[u8] = b"250 OK\r\n";
const MSG_354_NEXT_DATA: &[u8] = b"354 Start mail input; end with <CRLF>.<CRLF>\r\n";
const MSG_500_LENGTH_TOO_LONG: &[u8] = b"500 Line too long.\r\n";
const MSG_502_NOT_IMPLEMENTED: &[u8] = b"502 Command not implemented\r\n";
const MSG_503_BAD_SEQUENCE: &[u8] = b"503 Bad sequence of commands\r\n";

pub async fn serve_smtp(
    port: u16,
    server_name: &str,
    mails: Sender<Mail>,
    use_starttls: bool,
) -> crate::Result<()> {
    let addr: Vec<_> = format!("localhost:{}", port)
        .to_socket_addrs()
        .await?
        .collect();

    addr.iter()
        .map(|a| handle(a, server_name, mails.clone(), use_starttls))
        .collect::<FuturesUnordered<_>>()
        .skip_while(|r| future::ready(r.is_ok()))
        .take(1)
        .fold(Ok(()), |acc, cur| async {
            match cur {
                Err(e) => Err(e),
                Ok(()) => acc,
            }
        })
        .await
}

async fn handle(
    addr: &SocketAddr,
    server_name: &str,
    mails: Sender<Mail>,
    use_starttls: bool,
) -> crate::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Unable to bind {:?}: {}", addr, e))?;

    let mut incoming = listener.incoming();
    info!("SMTP listening on {:?}", listener.local_addr()?);

    while let Some(stream) = incoming.next().await {
        let conn = if let Ok(ref stream) = stream {
            ConnectionInfo::new(stream.local_addr().ok(), stream.peer_addr().ok())
        } else {
            ConnectionInfo::default()
        };
        let stream = stream?;
        info!("Accepting new connection from: {}", stream.peer_addr()?);
        spawn_task_and_swallow_log_errors(
            format!("Task: TCP transmission {}", conn),
            smtp_handle(
                stream,
                conn,
                server_name.to_string(),
                use_starttls,
                mails.clone(),
            ),
        );
    }
    Ok(())
}

async fn smtp_handle(
    stream: TcpStream,
    conn: ConnectionInfo,
    server_name: String,
    use_starttls: bool,
    mails: Sender<Mail>,
) -> crate::Result<()> {
    let mut smtp = Smtp::new(&stream, server_name, use_starttls);

    smtp.send_server_name().await?;

    let reader = BufReader::new(&stream);
    let mut lines = reader.lines();

    // Begin command loop
    while let Some(line) = lines.next().await {
        let action = smtp.process_line(Cow::Owned(line?));
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

struct Smtp<'a> {
    server_name: String,
    write_stream: TcpStream,
    use_starttls: bool,
    remote_name: Option<String>,
    addr_from: Option<String>,
    addr_to: Vec<String>,
    receive_data: bool,
    data: Cow<'a, str>,
}

impl<'a> Smtp<'a> {
    pub fn new(stream: &TcpStream, server_name: String, use_starttls: bool) -> Smtp<'a> {
        Self {
            server_name,
            write_stream: stream.clone(),
            use_starttls,
            remote_name: None,
            addr_from: None,
            addr_to: vec![],
            receive_data: false,
            data: Default::default(),
        }
    }

    async fn write(&mut self, messsage: &[u8]) -> crate::Result<()> {
        self.write_stream.write_all(messsage).await?;
        Ok(())
    }

    pub async fn send_server_name(&mut self) -> crate::Result<()> {
        // Write greeting server string
        self.write(format!("220 {} ESMTP\r\n", self.server_name).as_bytes())
            .await?;
        Ok(())
    }

    pub fn process_line(&self, command_line: Cow<'a, str>) -> Command<'a> {
        // debug!("texte: {}", line);
        if !self.receive_data {
            match command_line.to_lowercase().as_str() {
                "data" => Command::DataStart,
                "rset" => Command::Reset,
                "quit" => Command::Quit,
                "starttls" if self.use_starttls => Command::StartTls,
                // Helo
                line if line.len() > 5 && &line[..5] == "helo " => {
                    Command::Hello(command_line[5..].to_string())
                }
                // Ehlo
                line if line.len() > 5 && &line[..5] == "ehlo " => {
                    Command::Ehllo(command_line[5..].to_string())
                }
                // From
                line if line.len() > 10 && &line[..10] == "mail from:" => {
                    Command::From(command_line[10..].to_string())
                }
                // To
                line if line.len() > 8 && &line[..8] == "rcpt to:" => {
                    Command::Recipient(command_line[8..].to_string())
                }
                // Noop
                line if (line.len() == 4 && line == "noop")
                    || (line.len() > 4 && &line[..5] == "noop ") =>
                {
                    Command::Noop
                }
                // Anything else
                _ => Command::Error(command_line.into()),
            }
        } else if &command_line == "." {
            Command::DataEnd
        } else {
            Command::Data(command_line)
        }
    }

    pub fn reset(&mut self) {
        self.data.to_mut().clear();
        self.receive_data = false;
        self.addr_to.clear();
        self.remote_name = None;
        self.addr_from = None;
    }

    fn push_data(&mut self, line: Cow<'a, str>) {
        let idx = if !line.is_empty() && &line[0..1] == "." {
            1
        } else {
            0
        };
        if !self.data.is_empty() {
            self.data.to_mut().push_str("\r\n");
        }
        self.data.to_mut().push_str(&line[idx..]);
    }

    pub fn is_valid(&self, action: &Command) -> bool {
        match action {
            Command::Hello(_) | Command::Ehllo(_) => {
                self.addr_from.is_none() && self.addr_to.is_empty()
            }
            Command::From(_) => self.remote_name.is_some() && self.addr_to.is_empty(),
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

    pub async fn process_command(
        &mut self,
        command: Command<'a>,
    ) -> crate::Result<(bool, Option<Mail>)> {
        match command {
            action if !self.is_valid(&action) => {
                self.write(MSG_503_BAD_SEQUENCE).await?;
                Ok((true, None))
            }
            Command::Noop => {
                self.write(MSG_250_OK).await?;
                Ok((true, None))
            }
            Command::Ehllo(remote_name) | Command::Hello(remote_name) => {
                self.remote_name = Some(remote_name);
                let greeting = if self.use_starttls {
                    format!("250-{}\r\n250 STARTTLS\r\n", self.server_name)
                } else {
                    format!("250 {}\r\n", self.server_name)
                };
                self.write(greeting.as_bytes()).await?;
                Ok((true, None))
            }
            Command::StartTls => {
                // TODO: need to implement it
                unimplemented!();
                Ok((true, None))
            }
            Command::Reset => {
                self.reset();
                self.write(MSG_250_OK).await?;
                Ok((true, None))
            }
            Command::From(from) => {
                if from.len() > 64 {
                    error!("Username too long.");
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                } else {
                    self.addr_from = Some(from);
                    self.write(MSG_250_OK).await?
                }
                Ok((true, None))
            }
            Command::Recipient(to) => {
                self.addr_to.push(to);
                self.write(MSG_250_OK).await?;
                Ok((true, None))
            }
            Command::DataStart => {
                self.receive_data = true;
                self.write(MSG_354_NEXT_DATA).await?;
                Ok((true, None))
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
                Ok((true, None))
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
                self.data.to_mut().clear();

                self.write(MSG_250_OK).await?;
                Ok((true, Some(mail.clone())))
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
                Ok((false, None))
            }
            Command::Error(err) => {
                error!("Unsupported command: \"{}\"", err);

                self.write(MSG_502_NOT_IMPLEMENTED).await?;
                Ok((true, None))
            }
        }
    }
}
