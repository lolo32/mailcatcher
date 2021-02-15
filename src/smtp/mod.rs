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

use crate::{
    mail::Mail,
    smtp::command::Command,
    utils::{spawn_task_and_swallow_log_errors, ConnectionInfo},
};

/// SMTP command enum
mod command;

/// SMTP return message: All is alright, please continue
const MSG_250_OK: &[u8] = b"250 OK\r\n";
/// SMTP return message: DATA accepted, starting mail content
const MSG_354_NEXT_DATA: &[u8] = b"354 Start mail input; end with <CRLF>.<CRLF>\r\n";
/// SMTP return message: Line length is too long
const MSG_500_LENGTH_TOO_LONG: &[u8] = b"500 Line too long.\r\n";
/// SMTP return message: Command not understood
const MSG_502_NOT_IMPLEMENTED: &[u8] = b"502 Command not implemented\r\n";
/// SMTP return message: Command not allowed here
const MSG_503_BAD_SEQUENCE: &[u8] = b"503 Bad sequence of commands\r\n";

/// Serve SMTP
pub async fn serve(
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
        .map(|listener| accept_loop(listener, server_name, mails_broker.clone(), use_starttls))
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
#[allow(clippy::panic)]
fn bind(addr: &SocketAddr) -> TcpListener {
    // Bind to the address
    match task::block_on(async move {
        TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Unable to bind {:?}: {}", addr, e))
    }) {
        Ok(socket) => socket,
        Err(e) => {
            panic!("{:?}", e)
        }
    }
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
    log::info!("SMTP listening on {:?}", listener.local_addr()?);

    // For each new connection
    loop {
        if let Some(stream) = incoming.next().await {
            // Retrieve the Stream
            let stream = stream?;
            // New connection for information
            let conn: ConnectionInfo =
                ConnectionInfo::new(stream.local_addr().ok(), stream.peer_addr().ok());
            log::info!("Accepting new connection from: {}", stream.peer_addr()?);
            // Spawn local processing
            let _smtp_processing_task = spawn_task_and_swallow_log_errors(
                format!("Task: TCP transmission {}", conn),
                connection_loop(
                    stream,
                    conn,
                    server_name.to_owned(),
                    use_starttls,
                    mails_broker.clone(),
                ),
            )?;
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
        log::trace!("{:?}", action);
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

    log::info!(">>> {}", conn);

    Ok(())
}

/// SMTP transaction internal state
struct Smtp<'a, S: AsyncRead + AsyncWrite + Send + Sync + Unpin + Clone> {
    /// Server name identification (=my name)
    server_name: String,
    /// Stream where to write responses
    write_stream: S,
    /// Use TLS for connection support
    use_starttls: bool,
    /// Reported remote client name
    remote_name: Option<String>,
    /// Expeditor mail address
    addr_from: Option<String>,
    /// Recipient(s) address
    addr_to: Vec<String>,
    /// Are we in data reception or not
    receive_data: bool,
    /// Received data
    data: Cow<'a, str>,
}

#[allow(unused_lifetimes)]
impl<'a, S: AsyncRead + AsyncWrite + Send + Sync + Unpin + Clone> Smtp<'a, S> {
    /// New connection
    pub fn new(stream: &S, server_name: String, use_starttls: bool) -> Smtp<'a, S> {
        Self {
            server_name,
            write_stream: stream.clone(),
            use_starttls,
            remote_name: None,
            addr_from: None,
            addr_to: Vec::new(),
            receive_data: false,
            data: Cow::default(),
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
    #[allow(clippy::indexing_slicing)]
    pub fn process_line(&self, command_line: Cow<'a, str>) -> Command<'a> {
        log::debug!("texte: {}", command_line);
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
                    Command::From(command_line[10..].trim_start().to_owned())
                }
                // To
                line if line.len() > 8 && &line[..8] == "rcpt to:" => {
                    Command::Recipient(command_line[8..].trim_start().to_owned())
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
    #[allow(clippy::indexing_slicing)]
    fn push_data(&mut self, line: &str) {
        let start_idx: usize = if !line.is_empty() && &line[0..1] == "." {
            1
        } else {
            0
        };
        if !self.data.is_empty() {
            self.data.to_mut().push_str("\r\n");
        }
        self.data.to_mut().push_str(&line[start_idx..]);
    }

    /// return if the command is valid at this time of the speak
    pub fn is_valid(&self, action: &Command) -> bool {
        match *action {
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
            Command::Noop
            | Command::Quit
            | Command::Reset
            | Command::DataEnd
            | Command::Error(_) => true,
            // Only valid if specified at command line option, invalid otherwise
            Command::StartTls if self.use_starttls => true,
            Command::StartTls => false,
        }
    }

    /// Process command and value
    pub async fn process_command(&mut self, command: &Command<'a>) -> crate::Result<Option<Mail>> {
        log::debug!("{:?}", command);
        #[allow(clippy::pattern_type_mismatch, clippy::unimplemented)]
        match command {
            // Check if command is valid at this time of speaking
            action if !self.is_valid(action) => {
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
                    log::error!("Username too long.");
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
                    log::error!(
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
                log::trace!("{}", self.data);
                // Instantiate a new mail
                let mail: Mail = Mail::new(
                    self.addr_from.as_ref().ok_or("No sender mail address")?,
                    &self.addr_to,
                    &self.data,
                );

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
                log::error!("Unsupported command: \"{}\"", err);

                self.write(MSG_502_NOT_IMPLEMENTED).await?;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use async_std::{
        channel::bounded,
        net::{TcpListener, TcpStream},
        prelude::FutureExt,
    };
    use futures::io::Lines;

    use crate::mail::Type;

    use super::*;
    use async_std::channel::Receiver;
    use futures::TryFutureExt;

    async fn connect_to(port: u16) -> crate::Result<(Lines<BufReader<TcpStream>>, TcpStream)> {
        let stream: TcpStream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

        let reader: BufReader<TcpStream> = BufReader::new(stream.clone());
        let lines: Lines<BufReader<TcpStream>> = reader.lines();

        Ok((lines, stream))
    }

    #[test]
    #[allow(clippy::too_many_lines, clippy::indexing_slicing)]
    fn invalid_smtp_commands() -> std::io::Result<()> {
        const MY_NAME: &str = "UnitTest";

        async fn the_test(port: u16, my_name: &str) -> crate::Result<()> {
            let (mut lines, mut stream) = connect_to(port).await?;

            // Check if greeting is sent by the server
            log::trace!("First connecting");
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line[..(4 + my_name.len())], format!("220 {}", my_name));

            // --------------------------
            // Nothing is accepted but Helo, Ehlo, Reset or Noop
            log::trace!("INVALID");
            stream.write_all(b"INVALID\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "502 Command not implemented".to_owned());

            log::trace!("MAIL FROM first");
            stream
                .write_all(b"MAIL FROM:<test@example.org>\r\n")
                .await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "503 Bad sequence of commands".to_owned());

            log::trace!("RCPT TO first");
            stream.write_all(b"RCPT TO:<test@example.org>\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "503 Bad sequence of commands".to_owned());

            log::trace!("DATA first");
            stream.write_all(b"DATA\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "503 Bad sequence of commands".to_owned());

            log::trace!("NOOP");
            stream.write_all(b"NOOP\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            log::trace!("NOOP with message");
            stream
                .write_all(b"NOOP ignore the end of the line\r\n")
                .await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            log::trace!("RESET without upcase");
            stream.write_all(b"rSET\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            log::trace!("HELO");
            stream.write_all(b"HELO client\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, format!("250 {}", my_name));

            drop(lines);
            drop(stream);

            Ok(())
        }

        crate::test::log_init();

        let listener = crate::test::with_timeout(
            1_000,
            TcpListener::bind("127.0.0.1:0").map_err(|e| e.into()),
        )?;
        let port: u16 = listener.local_addr()?.port();

        let (sender, _receiver): crate::Channel<Mail> = bounded(1);

        crate::test::with_timeout(
            5_000,
            accept_loop(listener, MY_NAME, sender, false).race(the_test(port, MY_NAME)),
        )
    }

    #[test]
    #[allow(clippy::too_many_lines, clippy::indexing_slicing)]
    fn valid_smtp_commands() -> std::io::Result<()> {
        const MY_NAME: &str = "UnitTest";

        async fn the_test(
            port: u16,
            my_name: &str,
            mut receiver: Receiver<Mail>,
        ) -> crate::Result<()> {
            // --------------------------
            // Second try as ehlo
            log::trace!("Second connection");
            let (mut lines, mut stream) = connect_to(port).await?;

            // Greeting
            log::trace!("waiting greeting");
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, format!("220 {} ESMTP", my_name));

            log::trace!("EHLO");
            stream.write_all(b"eHLO client\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, format!("250 {}", my_name));

            // --------------------------
            // From
            log::trace!("MAIL FROM");
            stream
                .write_all(b"mAiL frOM:<from@example.org>\r\n")
                .await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            // --------------------------
            // To
            log::trace!("RCPT TO 1");
            stream.write_all(b"RCpT tO:<to@example.net>\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            log::trace!("RCPT TO 2");
            stream.write_all(b"rcpt TO:<to@example.org>\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK".to_owned());

            // --------------------------
            // Begin data
            log::trace!("DATA");
            stream.write_all(b"DATA\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(&line[..4], "354 ");

            log::trace!("Mail content");
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
            log::trace!("End of mail content");
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(line, "250 OK");

            // --------------------------
            // Check the received mail
            log::trace!("Mail received");
            let mail: Mail = receiver.next().await.ok_or("no next line")?;

            // --------------------------
            // Close connection
            log::trace!("QUIT");
            stream.write_all(b"quit\r\n").await?;
            let line = lines.next().await.ok_or("no next line")??;
            assert_eq!(
                line[..(4 + my_name.len())].to_string(),
                format!("221 {}", my_name)
            );

            log::trace!("Check mail received");
            let raw = mail.get_data(&Type::Raw).ok_or("no next line")?;
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
        }

        crate::test::log_init();

        let listener: TcpListener = crate::test::with_timeout(
            1_000,
            TcpListener::bind("127.0.0.1:0").map_err(|e| e.into()),
        )?;
        let port: u16 = listener.local_addr()?.port();

        let (sender, receiver): crate::Channel<Mail> = bounded(1);

        crate::test::with_timeout(
            5_000,
            accept_loop(listener, MY_NAME, sender, false).race(the_test(port, MY_NAME, receiver)),
        )
    }
}
