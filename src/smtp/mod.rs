use std::borrow::Cow;

use async_std::{
    channel::Sender,
    io::BufReader,
    net::{Incoming, SocketAddr, TcpListener, ToSocketAddrs},
    task,
};
use futures::{
    stream::FuturesUnordered,
    AsyncRead, AsyncWrite, {future, AsyncBufReadExt, AsyncWriteExt, StreamExt},
};
use log::{error, info, trace};

use crate::{
    mail::Mail,
    smtp::command::Command,
    utils::{spawn_task_and_swallow_log_errors, ConnectionInfo},
};

mod command;

const MSG_250_OK: &[u8] = b"250 OK\r\n";
const MSG_354_NEXT_DATA: &[u8] = b"354 Start mail input; end with <CRLF>.<CRLF>\r\n";
const MSG_500_LENGTH_TOO_LONG: &[u8] = b"500 Line too long.\r\n";
const MSG_502_NOT_IMPLEMENTED: &[u8] = b"502 Command not implemented\r\n";
const MSG_503_BAD_SEQUENCE: &[u8] = b"503 Bad sequence of commands\r\n";

/// Serve SMTP
pub async fn serve_smtp(
    port: u16,
    server_name: &str,
    mails_broker: Sender<Mail>,
    use_starttls: bool,
) -> crate::Result<()> {
    // Convert port to socket address
    let addr: Vec<SocketAddr> = format!("localhost:{}", port)
        .to_socket_addrs()
        .await?
        .collect();

    // For each socket address IPv4/IPv6 ...
    addr.iter()
        // ... bind TCP port for each address ...
        .map(bind)
        // ... spawn a handler to process incoming connection
        .map(|listener| {
            accept_loop(
                listener.unwrap(),
                server_name,
                mails_broker.clone(),
                use_starttls,
            )
        })
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

/// Handler that deals to a single socket address
fn bind(addr: &SocketAddr) -> crate::Result<TcpListener> {
    // Bind to the address
    task::block_on(async move {
        TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Unable to bind {:?}: {}", addr, e).into())
    })
}

/// Handler that deals to a single socket address
async fn accept_loop(
    listener: TcpListener,
    server_name: &str,
    mails_broker: Sender<Mail>,
    use_starttls: bool,
) -> crate::Result<()> {
    // Listen to incoming connection
    let mut incoming: Incoming = listener.incoming();
    info!("SMTP listening on {:?}", listener.local_addr()?);

    // For each new connection
    loop {
        if let Some(stream) = incoming.next().await {
            let stream = stream?;
            let conn: ConnectionInfo =
                ConnectionInfo::new(stream.local_addr().ok(), stream.peer_addr().ok());
            info!("Accepting new connection from: {}", stream.peer_addr()?);
            // Spawn local processing
            let _ = spawn_task_and_swallow_log_errors(
                format!("Task: TCP transmission {}", conn),
                connection_loop(
                    stream,
                    conn,
                    server_name.to_string(),
                    use_starttls,
                    mails_broker.clone(),
                ),
            );
        }
    }
}

/// Deals with each new connection
async fn connection_loop<S>(
    stream: S,
    conn: ConnectionInfo,
    server_name: String,
    use_starttls: bool,
    mails_broker: Sender<Mail>,
) -> crate::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Sync + Unpin + Clone,
{
    // Initialize the SMTP connection
    let mut smtp = Smtp::new(&stream, server_name, use_starttls);

    // Send SMTP banner to client
    smtp.send_server_name().await?;

    // Generate a line reader to process commands
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    // Begin command loop
    while let Some(line) = lines.next().await {
        // Process a new command line
        let line: String = line?;
        // Identify the action
        let action: Command = smtp.process_line(Cow::Owned(line));
        trace!("{:?}", action);
        // Process the action
        let mail: Option<Mail> = smtp.process_command(&action).await?;
        // If a mail has been emitted, send it to the HTTP side
        if let Some(mail) = mail {
            mails_broker.send(mail).await?;
        };
        // If the command ask to quit, exit the command processing
        if let Command::Quit = action {
            break;
        }
    }

    info!(">>> {}", conn);

    Ok(())
}

struct Smtp<'a, S: AsyncRead + AsyncWrite + Send + Sync + Unpin + Clone> {
    server_name: String,
    write_stream: S,
    use_starttls: bool,
    remote_name: Option<String>,
    addr_from: Option<String>,
    addr_to: Vec<String>,
    receive_data: bool,
    data: Cow<'a, str>,
}

impl<'a, S: AsyncRead + AsyncWrite + Send + Sync + Unpin + Clone> Smtp<'a, S> {
    pub fn new(stream: &S, server_name: String, use_starttls: bool) -> Smtp<'a, S> {
        Self {
            server_name,
            write_stream: stream.clone(),
            use_starttls,
            remote_name: None,
            addr_from: None,
            addr_to: Vec::new(),
            receive_data: false,
            data: Default::default(),
        }
    }

    /// Write response data to the client
    async fn write(&mut self, message: &[u8]) -> crate::Result<()> {
        self.write_stream.write_all(message).await?;
        Ok(())
    }

    /// Write the greeting message
    pub async fn send_server_name(&mut self) -> crate::Result<()> {
        // Write greeting server string
        self.write(format!("220 {} ESMTP\r\n", self.server_name).as_bytes())
            .await?;
        Ok(())
    }

    /// process client input, and return the command used
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
                    Command::From(command_line[10..].trim_start().to_string())
                }
                // To
                line if line.len() > 8 && &line[..8] == "rcpt to:" => {
                    Command::Recipient(command_line[8..].trim_start().to_string())
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

    /// Reset the data state
    pub fn reset(&mut self) {
        self.data.to_mut().clear();
        self.receive_data = false;
        self.addr_to.clear();
        self.remote_name = None;
        self.addr_from = None;
    }

    /// Store a new line, removing any one dot at the beginning of a line
    fn push_data(&mut self, line: &str) {
        let idx: usize = if !line.is_empty() && &line[0..1] == "." {
            1
        } else {
            0
        };
        if !self.data.is_empty() {
            self.data.to_mut().push_str("\r\n");
        }
        self.data.to_mut().push_str(&line[idx..]);
    }

    /// return if the command is valid at this time of the speak
    pub fn is_valid(&self, action: &Command) -> bool {
        match action {
            // Must be come first, so anything must be empty
            Command::Hello(_) | Command::Ehllo(_) => {
                self.addr_from.is_none() && self.addr_to.is_empty()
            }
            // Remote server name is specified, no recipient
            Command::From(_) => self.remote_name.is_some() && self.addr_to.is_empty(),
            // A server name AND an expeditor
            Command::Recipient(_) => self.remote_name.is_some() && self.addr_from.is_some(),
            // Recipient has been used
            Command::Data(_) | Command::DataStart => {
                self.remote_name.is_some() && self.addr_from.is_some() && !self.addr_to.is_empty()
            }
            // Always valid at anytime
            Command::Noop | Command::Quit | Command::Reset => true,
            Command::DataEnd | Command::Error(_) => true,
            // Only valid if specified at command line option, invalid otherwise
            Command::StartTls if self.use_starttls => true,
            Command::StartTls => false,
        }
    }

    /// Process command and value
    pub async fn process_command(&mut self, command: &Command<'a>) -> crate::Result<Option<Mail>> {
        match command {
            // Check if command is valid at this time of speaking
            action if !self.is_valid(&action) => {
                self.write(MSG_503_BAD_SEQUENCE).await?;
                Ok(None)
            }
            // Do nothing
            Command::Noop => {
                self.write(MSG_250_OK).await?;
                Ok(None)
            }
            // The client is greeting to the server, so indicate if starttls is supported or not
            Command::Ehllo(remote_name) | Command::Hello(remote_name) => {
                self.remote_name = Some(remote_name.clone());
                let greeting: String = if self.use_starttls {
                    format!("250-{}\r\n250 STARTTLS\r\n", self.server_name)
                } else {
                    format!("250 {}\r\n", self.server_name)
                };
                self.write(greeting.as_bytes()).await?;
                Ok(None)
            }
            // WiP
            Command::StartTls => {
                // TODO: need to implement it
                unimplemented!();
            }
            // Reset the state
            Command::Reset => {
                self.reset();
                self.write(MSG_250_OK).await?;
                Ok(None)
            }
            // Store the expeditor address
            Command::From(from) => {
                if from.len() > 64 {
                    error!("Username too long.");
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                } else {
                    self.addr_from = Some(from.clone());
                    self.write(MSG_250_OK).await?
                }
                Ok(None)
            }
            // Store the recipient addresses
            Command::Recipient(to) => {
                self.addr_to.push(to.clone());
                self.write(MSG_250_OK).await?;
                Ok(None)
            }
            // DATA command sent, so entering data mode
            Command::DataStart => {
                self.receive_data = true;
                self.write(MSG_354_NEXT_DATA).await?;
                Ok(None)
            }
            // Receive data, store it if line length is valid
            Command::Data(line) => {
                if line.len() > 1000 {
                    error!(
                        "Data line length ({}) cannot exceed 998 characters.",
                        line.len()
                    );
                    self.write(MSG_500_LENGTH_TOO_LONG).await?
                }
                self.push_data(line);
                Ok(None)
            }
            // A line containing only "." specified, so mail is complete
            Command::DataEnd => {
                trace!("{}", self.data);
                // Instantiate a new mail
                let mail: Mail =
                    Mail::new(self.addr_from.as_ref().unwrap(), &self.addr_to, &self.data);

                self.receive_data = false;
                self.addr_from = None;
                self.addr_to.clear();
                self.data.to_mut().clear();

                self.write(MSG_250_OK).await?;
                Ok(Some(mail))
            }
            // Exit the connection
            Command::Quit => {
                self.write(
                    format!(
                        "221 {} Service closing transmission channel\r\n",
                        self.server_name
                    )
                    .as_bytes(),
                )
                .await?;
                Ok(None)
            }
            // An error message, because the command is not supported
            Command::Error(err) => {
                error!("Unsupported command: \"{}\"", err);

                self.write(MSG_502_NOT_IMPLEMENTED).await?;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_std::{
        channel::bounded,
        net::{TcpListener, TcpStream},
        prelude::FutureExt,
        task,
    };
    use futures::io::Lines;

    use crate::mail::Type;

    use super::*;

    async fn connect_to(port: u16) -> crate::Result<(Lines<BufReader<TcpStream>>, TcpStream)> {
        let stream: TcpStream = TcpStream::connect(format!("localhost:{}", port)).await?;

        let reader: BufReader<TcpStream> = BufReader::new(stream.clone());
        let lines: Lines<BufReader<TcpStream>> = reader.lines();

        Ok((lines, stream))
    }

    #[test]
    fn test_smtp() {
        async fn async_test() -> crate::Result<()> {
            const MY_NAME: &str = "UnitTest";

            let tcp: TcpListener = TcpListener::bind("localhost:0").await?;
            let port: u16 = tcp.local_addr().unwrap().port();
            drop(tcp);

            let (sender, mut receiver): crate::Channel<Mail> = bounded(1);

            let serve = serve_smtp(port, MY_NAME, sender, false);

            let fut = async move {
                let (mut lines, mut stream) = connect_to(port).await?;

                // Check if greeting is sent by the server
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line[..(4 + MY_NAME.len())], format!("220 {}", MY_NAME));

                // --------------------------
                // Nothing is accepted but Helo, Ehlo, Reset or Noop
                stream.write_all(b"INVALID\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "502 Command not implemented".to_string());

                stream
                    .write_all(b"MAIL FROM:<test@example.org>\r\n")
                    .await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "503 Bad sequence of commands".to_string());

                stream.write_all(b"RCPT TO:<test@example.org>\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "503 Bad sequence of commands".to_string());

                stream.write_all(b"DATA\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "503 Bad sequence of commands".to_string());

                stream.write_all(b"NOOP\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                stream
                    .write_all(b"NOOP ignore the end of the line\r\n")
                    .await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                stream.write_all(b"rSET\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                stream.write_all(b"HELO client\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, format!("250 {}", MY_NAME));

                drop(lines);
                drop(stream);

                // --------------------------
                // Second try as ehlo
                let (mut lines, mut stream) = connect_to(port).await?;

                // Greeting
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, format!("220 {} ESMTP", MY_NAME));

                stream.write_all(b"eHLO client\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, format!("250 {}", MY_NAME));

                // --------------------------
                // From
                stream
                    .write_all(b"mAiL frOM:<from@example.org>\r\n")
                    .await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                // --------------------------
                // To
                stream.write_all(b"RCpT tO:<to@example.net>\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                stream.write_all(b"rcpt TO:<to@example.org>\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK".to_string());

                // --------------------------
                // Begin data
                stream.write_all(b"DATA\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(&line[..4], "354 ");

                stream
                    .write_all(
                        b"From: =?US-ASCII?Q?Keith_Moore?= <moore@cs.utk.edu>;\r\n\
To: =?ISO-8859-1?Q?Keld_J=F8rn_Simonsen?= <keld@dkuug.dk>;\r\n\
CC: =?ISO-8859-1?Q?Andr=E9?= Pirard <PIRARD@vm1.ulg.ac.be>;\r\n\
Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?=\r\n\
 =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=\r\n\
\r\n\
.This is the content of this mail... but it says nothing now.\r\n\
\r\n\
.\r\n",
                    )
                    .await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(line, "250 OK");

                // --------------------------
                // Check the received mail
                let mail: Mail = receiver.next().await.unwrap();

                // --------------------------
                // Close connection
                stream.write_all(b"quit\r\n").await?;
                let line = lines.next().await.unwrap();
                let line: String = line?;
                assert_eq!(
                    line[..(4 + MY_NAME.len())].to_string(),
                    format!("221 {}", MY_NAME)
                );

                let raw = mail.get_data(&Type::Raw).unwrap();
                assert_eq!(
                    raw,
                    "From: =?US-ASCII?Q?Keith_Moore?= <moore@cs.utk.edu>;\r\n\
To: =?ISO-8859-1?Q?Keld_J=F8rn_Simonsen?= <keld@dkuug.dk>;\r\n\
CC: =?ISO-8859-1?Q?Andr=E9?= Pirard <PIRARD@vm1.ulg.ac.be>;\r\n\
Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?=\r\n\
 =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=\r\n\
\r\n\
This is the content of this mail... but it says nothing now.\r\n"
                );

                Ok(())
            };

            serve.try_race(fut).await
        }

        task::block_on(async {
            async_test()
                .timeout(Duration::from_millis(5000))
                .await
                .unwrap()
                .unwrap()
        })
    }
}
