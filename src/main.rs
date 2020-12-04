#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use async_std::task;
use futures_lite::FutureExt;

mod encoding;
mod http;
mod mail;
mod smtp;
mod utils;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> crate::Result<()> {
    env_logger::init();
    task::block_on(main_fut())
}

async fn main_fut() -> crate::Result<()> {
    // TODO: Parse arguments
    let (port_smtp, port_http) = (1025_u16, 1080_u16);
    // TODO: Parse arguments
    let my_name = "MailCatcher".to_string();
    // TODO: Parse argument
    let use_starttls = false;

    info!(
        "Starting MailCatcher on port smtp({}) and http({})",
        port_smtp, port_http
    );

    let (tx_mails, rx_mails) = async_channel::unbounded();

    let s = smtp::serve_smtp(port_smtp, my_name.clone(), tx_mails, use_starttls);
    let h = http::serve_http(port_http, my_name.clone(), rx_mails);
    let _a = s.race(h).await?;

    Ok(())
}
