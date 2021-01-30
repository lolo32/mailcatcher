use std::borrow::Cow;

/// SMTP command supported
#[derive(Debug)]
pub enum Command<'a> {
    /// HELO
    Hello(String),
    /// EHLO
    Ehllo(String),
    /// STARTTLS
    StartTls,
    /// MAIL FROM:
    From(String),
    /// RCPT TO:
    Recipient(String),
    /// DATA
    Data(Cow<'a, str>),
    /// Internal, data starting
    DataStart,
    /// Internal, data ended
    DataEnd,
    /// NOOP
    Noop,
    /// RSET
    Reset,
    /// QUIT
    Quit,
    /// An error with it's message
    Error(String),
}
