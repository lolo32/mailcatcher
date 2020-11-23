#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use async_std::task;

mod encoding;
mod mail;
mod smtp;
mod utils;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> Result<()> {
    env_logger::init();
    task::block_on(main_fut())
}

async fn main_fut() -> Result<()> {
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

    smtp::serve_smtp(port_smtp, my_name.clone(), use_starttls).await?;

    Ok(())
}
