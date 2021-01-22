use async_std::{
    channel::{self, Receiver, Sender},
    prelude::FutureExt,
    task,
};
use futures::StreamExt;
use log::{debug, error, info, trace};
use structopt::StructOpt;

use crate::{
    http::{bind_http, Params},
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

#[derive(Debug, StructOpt)]
#[structopt(about, author)]
struct Opt {
    /// SMTP listening port
    #[structopt(long, default_value = "1025")]
    smtp: u16,

    /// HTTP listening port
    #[structopt(long, default_value = "1080")]
    http: u16,

    /// Allow to use StartTls (not implemented)
    #[structopt(skip)]
    use_starttls: bool,

    /// Name to use in the SMTP hello
    ///
    /// This is the name that is used during the SMTP greeting sequence
    #[structopt(long, default_value = "MailCatcher")]
    smtp_name: String,

    /// Open browser's webpage at the start
    #[structopt(long)]
    browser: bool,
}

fn main() -> crate::Result<()> {
    // Initialize the log crate/macros based on RUST_LOG env value
    env_logger::init();

    let opt = Opt::from_args();
    debug!("Options: {:?}", opt);

    // Start the program, that is async, so block waiting it's end
    task::block_on(main_fut(opt))
}

async fn main_fut(opt: Opt) -> crate::Result<()> {
    info!(
        "Starting MailCatcher on port smtp({}) and http({})",
        opt.smtp, opt.http
    );

    // Channels used to notify a new mail arrived in SMTP side to HTTP side
    let (tx_mail_from_smtp, mut rx_mail_from_smtp): (Sender<Mail>, Receiver<Mail>) =
        channel::bounded(1);
    let (tx_mail_broker, rx_mail_broker) = channel::unbounded();

    let mail_broker = mail_broker(rx_mail_broker);

    let (tx_new_mail, rx_new_mail) = channel::bounded(1);
    let tx_http_new_mail = tx_mail_broker.clone();
    let http_params = Params {
        mail_broker: tx_mail_broker,
        rx_mails: rx_new_mail,
        #[cfg(feature = "faking")]
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
    let s = smtp::serve_smtp(
        opt.smtp,
        &opt.smtp_name,
        tx_mail_from_smtp,
        opt.use_starttls,
    );
    // Starting HTTP side
    let http_app = http::serve_http(http_params).await?;

    // Open browser window at start if specified
    if opt.browser {
        opener::open(format!("http://localhost:{}/", opt.http))?;
    }

    // Waiting for both to complete
    s.try_join(bind_http(http_app, opt.http))
        .try_join(mail_broker)
        .await?;

    unreachable!()
}
