use std::borrow::Cow;

#[derive(Debug)]
pub enum Command<'a> {
    Hello(String),
    Ehllo(String),
    StartTls,
    From(String),
    Recipient(String),
    Data(Cow<'a, str>),
    DataStart,
    DataEnd,
    Noop,
    Reset,
    Quit,
    Error(String),
}
