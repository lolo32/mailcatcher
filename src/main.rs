#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use async_std::{
    channel::{self, Receiver, Sender},
    prelude::FutureExt,
    task,
};
use futures::StreamExt;

use crate::{
    http::Params,
    mail::{
        broker::{mail_broker, MailEvt},
        Mail,
    },
    utils::spawn_task_and_swallow_log_errors,
};

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
    let (tx_mail_from_smtp, mut rx_mail_from_smtp): (Sender<Mail>, Receiver<Mail>) =
        channel::bounded(1);
    let (tx_mail_broker, rx_mail_broker) = channel::unbounded();

    let mail_broker = mail_broker(rx_mail_broker);

    let (tx_new_mail, rx_new_mail) = channel::bounded(1);
    let tx_http_new_mail = tx_mail_broker.clone();
    let http_params = Params {
        port: port_http,
        mail_broker: tx_mail_broker,
        rx_mails: rx_new_mail,
        #[cfg(feature = "fake")]
        tx_new_mail: tx_mail_from_smtp.clone(),
    };
    spawn_task_and_swallow_log_errors("Task: Mail notifier".into(), async move {
        loop {
            // To do on each received new mail
            if let Some(mail) = rx_mail_from_smtp.next().await {
                info!("Received new mail: {:?}", mail);
                // Notify javascript side by SSE
                match tx_http_new_mail.send(MailEvt::NewMail(mail.clone())).await {
                    Ok(_) => {
                        tx_new_mail.send(mail).await.unwrap();
                        trace!("Mail stored successfully")
                    }
                    Err(e) => error!("Mail stored error: {:?}", e),
                }
            }
        }
    });

    // Starting SMTP side
    let s = smtp::serve_smtp(port_smtp, my_name, tx_mail_from_smtp, use_starttls);
    // Starting HTTP side
    let h = http::serve_http(http_params);
    // Waiting for both to complete
    s.try_join(h).try_join(mail_broker).await?;

    unreachable!()
}
