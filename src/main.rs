#![deny(
    missing_copy_implementations,
    missing_docs,
    missing_debug_implementations,
    single_use_lifetimes,
    unsafe_code,
    unused_extern_crates,
    unused_import_braces,
    unused_lifetimes,
    unused_qualifications,
    unused_results,
    clippy::all,
    clippy::pedantic,
    clippy::nursery
)]
// Clippy rules in the `Restriction lints`
#![deny(
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::create_dir,
    clippy::dbg_macro,
    clippy::decimal_literal_representation,
    clippy::else_if_without_else,
    clippy::exit,
    clippy::filetype_is_file,
    clippy::float_arithmetic,
    clippy::float_cmp_const,
    clippy::get_unwrap,
    clippy::indexing_slicing,
    clippy::inline_asm_x86_att_syntax,
    clippy::inline_asm_x86_intel_syntax,
    clippy::integer_arithmetic,
    clippy::integer_division,
    clippy::let_underscore_must_use,
    clippy::lossy_float_literal,
    clippy::map_err_ignore,
    clippy::mem_forget,
    clippy::missing_docs_in_private_items,
    clippy::missing_inline_in_public_items,
    clippy::modulo_arithmetic,
    clippy::multiple_inherent_impl,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::pattern_type_mismatch,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::rc_buffer,
    clippy::rest_pat_in_fully_bound_structs,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::str_to_string,
    clippy::string_add,
    clippy::string_to_string,
    clippy::todo,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unwrap_in_result,
    clippy::unwrap_used,
    clippy::use_debug,
    clippy::verbose_file_reads,
    clippy::wildcard_enum_match_arm,
    clippy::wrong_pub_self_convention
)]

//! This software can be used to have a SMTP mail server on any computer that
//! accept any mail and display them using any web browser.
//!
//! It DOES NOT really send them to any remote recipient address.

use async_std::{
    channel::{self, Receiver, Sender},
    prelude::FutureExt,
    task,
};
use futures::StreamExt;
use structopt::StructOpt;
use tide::Server;

use crate::{
    http::{bind as bind_http, sse_evt::SseEvt, Params, State},
    mail::{
        broker::{process as mail_broker, MailEvt},
        Mail,
    },
    utils::spawn_task_and_swallow_log_errors,
};

/// Decode encoded string
mod encoding;
/// Display mail content with HTTP content
mod http;
/// Mail representation/gestion
mod mail;
/// SMTP part
mod smtp;
/// Deals with async tasks
mod utils;

/// Result type commonly used in this crate
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Type alias for stream channel
type Channel<T> = (Sender<T>, Receiver<T>);

/// Command line arguments
#[derive(Debug, StructOpt)]
#[structopt(about, author)]
struct Opt {
    /// SMTP listening port
    #[structopt(long, default_value = "1025")]
    smtp: u16,

    /// HTTP listening port
    #[structopt(long, default_value = "1080")]
    http: u16,

    /// Allow to use StartTls (not yet implemented!)
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

fn main() -> Result<()> {
    // Initialize the log crate/macros based on RUST_LOG env value
    env_logger::init();

    let opt: Opt = Opt::from_args();
    log::debug!("Options: {:?}", opt);

    // Start the program, that is async, so block waiting it's end
    task::block_on(main_fut(opt))
}

/// async main
async fn main_fut(opt: Opt) -> Result<()> {
    log::info!(
        "Starting MailCatcher on port smtp({}) and http({})",
        opt.smtp,
        opt.http
    );

    // Channels used to notify a new mail arrived in SMTP side to HTTP side
    let (tx_mail_from_smtp, mut rx_mail_from_smtp): Channel<Mail> = channel::bounded(1);
    let (tx_mail_broker, rx_mail_broker): Channel<MailEvt> = channel::unbounded();

    let mail_broker = mail_broker(rx_mail_broker);

    let (tx_new_mail, rx_new_mail): Channel<Mail> = channel::bounded(1);
    let tx_http_new_mail: Sender<MailEvt> = tx_mail_broker.clone();
    let http_params: Params = Params {
        mail_broker: tx_mail_broker,
        rx_mails: rx_new_mail,
        #[cfg(feature = "faking")]
        tx_new_mail: tx_mail_from_smtp.clone(),
    };
    let _mail_notifier_task =
        spawn_task_and_swallow_log_errors("Task: Mail notifier".into(), async move {
            loop {
                // To do on each received new mail
                if let Some(mail) = rx_mail_from_smtp.next().await {
                    log::info!("Received new mail: {:?}", mail);
                    // Notify javascript side by SSE
                    match tx_http_new_mail.send(MailEvt::NewMail(mail.clone())).await {
                        Ok(()) => {
                            tx_new_mail.send(mail).await?;
                            log::trace!("Mail stored successfully")
                        }
                        Err(e) => log::error!("Mail stored error: {:?}", e),
                    }
                }
            }
        })?;

    // Starting SMTP side
    let s = smtp::serve(
        opt.smtp,
        &opt.smtp_name,
        tx_mail_from_smtp,
        opt.use_starttls,
    );
    // Starting HTTP side
    let http_app: Server<State<SseEvt>> = http::init(http_params).await?;

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

#[cfg(test)]
mod test {
    pub fn log_init() {
        // Initialize the log crate/macros based on RUST_LOG env value
        let logger = femme::pretty::Logger::new();

        match async_log::Logger::wrap(logger, || 12).start(log::LevelFilter::Debug) {
            Ok(_) => {
                // Log initialisation OK
            }
            Err(_e) => {
                // Already initialized
            }
        }
    }
}
