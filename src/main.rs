#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use async_std::{channel, prelude::FutureExt, task};

mod encoding;
mod http;
mod mail;
mod smtp;
mod utils;

// Result type commonly used in this crate
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> crate::Result<()> {
    // Initialize the log crate/macros based on RUST_LOG env value
    env_logger::init();
    // Start the program, that is async, so block waiting it's end
    task::block_on(main_fut())
}

async fn main_fut() -> crate::Result<()> {
    // TODO: Parse arguments
    let (port_smtp, port_http) = (1025_u16, 1080_u16);
    // TODO: Parse arguments
    let my_name = "MailCatcher";
    // TODO: Parse argument
    let use_starttls = false;

    info!(
        "Starting MailCatcher on port smtp({}) and http({})",
        port_smtp, port_http
    );

    // Channels used to notify a new mail arrived in SMTP side to HTTP side
    let (tx_mails, rx_mails) = channel::unbounded();

    // Starting SMTP side
    let s = smtp::serve_smtp(port_smtp, my_name, tx_mails, use_starttls);
    // Starting HTTP side
    let h = http::serve_http(port_http, rx_mails);
    // Waiting for both to complete
    s.try_join(h).await?;

    unreachable!()
}
