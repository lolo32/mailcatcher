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
    setup_log();

    let opt: Opt = Opt::from_args();
    log::debug!("Options: {:?}", opt);

    // Start the program, that is async, so block waiting it's end
    task::block_on(main_fut(opt))
}

/// Log output configuration
fn setup_log() {
    let logger = femme::pretty::Logger::new();
    async_log::Logger::wrap(logger, || 12)
        .start(log::LevelFilter::Trace)
        .expect("async-log configured");
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

    let mail_broker = mail_broker(rx_mail_broker, "mail_broker");

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

    #[cfg(target_os = "windows")]
    windows::tray_icon()?;

    // Waiting for both to complete
    s.try_join(bind_http(http_app, opt.http))
        .try_join(mail_broker)
        .await?;

    unreachable!()
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows {
    // based on https://stackoverflow.com/questions/54047397/how-to-make-a-tray-icon-for-windows-using-the-winapi-crate

    //-----Import Libraries (called crates)-----
    use winapi;
    //-----Import Built-in Libraries (not called crates)-----
    use std::ffi::OsStr;
    //get size of stuff and init with zeros
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    //use cmd.exe
    use std::process::Command;
    //use a null pointer (I think)
    use std::ptr::null_mut;

    pub fn tray_icon() -> crate::Result<()> {
        // to navigate calling with the winapi "crate" use the search function at link
        // https://docs.rs/winapi/*/x86_64-pc-windows-msvc/winapi/um/wincon/fn.GetConsoleWindow.html

        //System Tray Icon support - here it is

        //prep WM_MYMESSAGE
        const WM_MYMESSAGE: u32 = winapi::um::winuser::WM_USER + 1;
        //fill with 0's
        let mut tray_tool_tip_int: [u16; 128] = [0; 128];
        //convert to OS string format or something
        let mut tray_tool_tip_step_os = OsStr::new("Tool tip words here");
        //now actually convert to UTF16 format for the OS
        let mut tray_tool_tip_step_utf16 =
            tray_tool_tip_step_os.encode_wide().collect::<Vec<u16>>();
        //record it in that nice integer holder
        tray_tool_tip_int[..tray_tool_tip_step_utf16.len()]
            .copy_from_slice(&tray_tool_tip_step_utf16);

        //thing that has info on window and system tray stuff in it
        let mut nid: winapi::um::shellapi::NOTIFYICONDATAW = unsafe { zeroed() };
        //prep
        nid.cbSize = size_of::<winapi::um::shellapi::NOTIFYICONDATAW>() as u32;
        //gets the current console window handle
        nid.hWnd = unsafe { winapi::um::wincon::GetConsoleWindow() };
        //it's a number
        nid.uID = 1001;
        //whoknows should be related to click capture but doesn't so
        nid.uCallbackMessage = WM_MYMESSAGE;
        //icon idk
        nid.hIcon = unsafe {
            winapi::um::winuser::LoadIconW(null_mut(), winapi::um::winuser::IDI_APPLICATION)
        };
        //tooltip for the icon
        nid.szTip = tray_tool_tip_int;
        //who knows
        nid.uFlags = winapi::um::shellapi::NIF_MESSAGE
            | winapi::um::shellapi::NIF_ICON
            | winapi::um::shellapi::NIF_TIP;

        //gets the size of nid.szTip (tooltip length) indirectly (not the right size!)
        //let mut nidsz_tip_length = tray_tool_tip.chars().count() as u64;
        //gets the size of nid.szTip (tooltip length) for the UTF-16 format, which is what Windows cares about
        let mut nidsz_tip_length = tray_tool_tip_step_utf16.len() as u64;

        //shows the icon
        let _ = unsafe {
            winapi::um::shellapi::Shell_NotifyIconW(winapi::um::shellapi::NIM_ADD, &mut nid)
        };
        let _ = Command::new("cmd.exe").arg("/c").arg("pause").status();

        //fill with 0's (clear it out I hope)
        tray_tool_tip_int = [0; 128];
        //convert to OS string format or something
        tray_tool_tip_step_os = OsStr::new("An updated tooltip is now here!");
        //now actually convert to UTF16 format for the OS
        tray_tool_tip_step_utf16 = tray_tool_tip_step_os.encode_wide().collect::<Vec<u16>>();
        //record it in that nice integer holder
        tray_tool_tip_int[..tray_tool_tip_step_utf16.len()]
            .copy_from_slice(&tray_tool_tip_step_utf16);
        //tooltip for the icon
        nid.szTip = tray_tool_tip_int;
        //gets the size of nid.szTip (tooltip length) indirectly (not the right size!)
        // nidsz_tip_length = tray_tool_tip.chars().count() as u64;
        //gets the size of nid.szTip (tooltip length) for the UTF-16 format, which is what Windows cares about
        nidsz_tip_length = tray_tool_tip_step_utf16.len() as u64;
        let _ = unsafe {
            //updates system tray icon
            winapi::um::shellapi::Shell_NotifyIconW(winapi::um::shellapi::NIM_MODIFY, &mut nid)
        };

        let _ = Command::new("cmd.exe").arg("/c").arg("pause").status();

        let _ = unsafe {
            //deletes system tray icon when done
            winapi::um::shellapi::Shell_NotifyIconW(winapi::um::shellapi::NIM_DELETE, &mut nid)
        };

        let _ = Command::new("cmd.exe").arg("/c").arg("pause").status();

        Ok(())
    }
}
