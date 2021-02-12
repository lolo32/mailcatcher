use std::time::Duration;

use async_std::{
    channel::{Receiver, Sender},
    net::{SocketAddr, ToSocketAddrs},
    task,
};
use broadcaster::BroadcastChannel;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    StreamExt,
};
use tide::{prelude::Listener, Server};

use crate::{
    http::sse_evt::SseEvt,
    mail::{broker::MailEvt, Mail},
    utils::spawn_task_and_swallow_log_errors,
};

/// Files in the "asset" directory
mod asset;
/// Routes initialisation
mod routes;
/// Server-Sent Events
mod sse;
/// Events sent by SSE
pub mod sse_evt;

/// Tide Connection State
#[derive(Clone)]
pub struct State<T>
where
    T: Send + Clone + 'static,
{
    /// Stream used for receiving SSE messages
    sse_stream: BroadcastChannel<T, UnboundedSender<T>, UnboundedReceiver<T>>,
    /// Mail broker storage stream
    mail_broker: Sender<MailEvt>,

    #[cfg(feature = "faking")]
    /// Send a new Fake new mail
    new_fake_mail: Sender<Mail>,
}

/// Parameters used to initialise the HTTP webserver side
pub struct Params {
    /// Sender stream to access the mail broker
    pub mail_broker: Sender<MailEvt>,
    /// Receiver stream of new mails added
    pub rx_mails: Receiver<Mail>,

    #[cfg(feature = "faking")]
    /// Sender stream to notify fake new mail
    pub tx_new_mail: Sender<Mail>,
}

/// Initialize the HTTP webserver
///
/// * Build SSE brokers
/// * Add routes
pub async fn init(params: Params) -> crate::Result<Server<State<SseEvt>>> {
    // Stream reader and writer for SSE notifications
    let sse_stream = BroadcastChannel::new();

    let sse_stream_new_mail = sse_stream.clone();
    let mut rx_mails: Receiver<Mail> = params.rx_mails;
    let _mail_notification_task =
        spawn_task_and_swallow_log_errors("Task: Mail notifier".into(), async move {
            loop {
                // To do on each received new mail
                if let Some(mail) = rx_mails.next().await {
                    log::info!(">>> Received new mail: {:?}", mail);
                    // Append the mail to the list
                    match sse_stream_new_mail.send(&SseEvt::NewMail(mail)).await {
                        Ok(()) => log::trace!(">>> New mail notification sent to channel"),
                        Err(e) => log::error!(">>> Err new mail notification to channel: {:?}", e),
                    }
                }
            }
        })?;

    // Noop consumer to empty the ctream
    let mut sse_noop_stream = sse_stream.clone();
    let _noop_task =
        spawn_task_and_swallow_log_errors("Task: Noop stream emptier".into(), async move {
            loop {
                log::trace!("Consume SSE notification stream");
                // Do nothing, it's just to empty the stream
                let _sse_evt = sse_noop_stream.next().await;
            }
        })?;

    // Task sending ping to SSE terminators
    let sse_tx_ping_stream = sse_stream.clone();
    let _sse_ping_task =
        spawn_task_and_swallow_log_errors("Task: Ping SSE sender".into(), async move {
            loop {
                log::trace!("Sending ping");
                // Do nothing, it's just to empty the stream
                sse_tx_ping_stream.send(&SseEvt::Ping).await?;
                task::sleep(Duration::from_secs(10)).await;
            }
        })?;

    let state: State<SseEvt> = State {
        sse_stream,
        mail_broker: params.mail_broker,
        #[cfg(feature = "faking")]
        new_fake_mail: params.tx_new_mail,
    };

    Ok(routes::init(state).await?)
}

/// Bind the initialised webserver to the port then listen to incoming connection
pub async fn bind<T>(app: Server<State<T>>, port: u16) -> crate::Result<()>
where
    T: Send + Clone + 'static,
{
    // Bind ports
    let mut listener = app
        .bind(
            format!("localhost:{}", port)
                .to_socket_addrs()
                .await?
                .collect::<Vec<SocketAddr>>(),
        )
        .await?;
    // Display binding ports
    for info in &listener.info() {
        log::info!("HTTP listening on {}", info);
    }
    // Accept connections
    listener.accept().await?;

    unreachable!()
}

#[cfg(test)]
mod tests {
    use std::env;

    use async_std::{
        channel,
        fs::File,
        path::{Path, PathBuf},
    };
    use futures::AsyncReadExt;
    use tide::{
        http::{headers, mime, Method, Request, Response, Url},
        prelude::{json, Deserialize, Serialize},
        StatusCode,
    };

    use crate::mail::{broker::process as mail_broker, HeaderRepresentation};

    use super::*;

    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct MailSummary {
        id: String,
        from: String,
        to: Vec<String>,
        subject: String,
        date: i64,
        size: usize,
    }

    fn init() {
        // Initialize the log crate/macros based on RUST_LOG env value
        match env_logger::try_init() {
            Ok(_) => {
                // Log initialisation OK
            }
            Err(_e) => {
                // Already initialized
            }
        }
    }

    fn get_asset_path() -> PathBuf {
        Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| {
            env::current_dir()
                .expect("get cwd")
                .to_str()
                .expect("to str")
                .to_owned()
        }))
        .join("asset")
    }

    #[test]
    #[allow(clippy::too_many_lines, clippy::panic)]
    fn test_routes() -> crate::Result<()> {
        async fn the_test() -> crate::Result<()> {
            let (tx_mail_broker, rx_mail_broker): crate::Channel<MailEvt> = channel::unbounded();
            let (_tx_new_mail, rx_new_mail): crate::Channel<Mail> = channel::bounded(1);
            #[cfg(feature = "faking")]
            let (tx_mail_from_faking, mut rx_mail_from_faking): (
                Sender<Mail>,
                Receiver<Mail>,
            ) = channel::unbounded();

            // Provide some mails
            let mut mails: Vec<Mail> = Vec::new();
            for _ in 0..10 {
                let mail: Mail = Mail::fake();
                mails.push(mail.clone());
                tx_mail_broker.send(MailEvt::NewMail(mail)).await?;
            }
            let _mail_broker_task = spawn_task_and_swallow_log_errors(
                "test_routes_mails".to_owned(),
                mail_broker(rx_mail_broker),
            );

            // Init the HTTP side
            let params: Params = Params {
                mail_broker: tx_mail_broker,
                rx_mails: rx_new_mail,
                #[cfg(feature = "faking")]
                tx_new_mail: tx_mail_from_faking,
            };
            let app: Server<State<SseEvt>> = super::init(params).await?;

            // Assets
            for (filename, mime_type) in vec![
                ("home.html", mime::HTML),
                ("w3.css", mime::CSS),
                ("hyperapp.js", mime::JAVASCRIPT),
            ] {
                let mut url = Url::parse("http://localhost/")?;
                if filename != "home.html" {
                    url.set_path(filename);
                };
                let request: Request = Request::new(Method::Get, url);
                let mut response: Response = app.respond(request).await?;
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .ok_or("Content-Type exists unavailable")?,
                    &mime_type.to_string()
                );
                let mut fs: File = File::open(get_asset_path().join(filename)).await?;
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await?;
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                assert_eq!(response.body_string().await?, home_content);
            }

            // Test deflate
            {
                let mut request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/")?);
                let _ = request.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
                let mut response: Response = app.respond(request).await?;
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .ok_or("Content-Type header unavailable")?,
                    &mime::HTML.to_string()
                );
                assert_eq!(
                    response
                        .header(headers::CONTENT_ENCODING)
                        .ok_or("Content-Encoding header unavailable")?,
                    "deflate"
                );
                let mut fs: File = File::open(get_asset_path().join("home.html")).await?;
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await?;
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                let res_content: Vec<u8> = response.body_bytes().await?;
                // Deflate the content
                let res_content: Vec<u8> = miniz_oxide::inflate::decompress_to_vec(&res_content)
                    .map_err(|e| format!("{:?}", e))?;
                let res_content: String = String::from_utf8(res_content)?;
                assert_eq!(res_content, home_content);
            }

            // Get all mails
            {
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mails")?);
                let mut response: Response = app.respond(request).await?;
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .ok_or("Content-Type header unavailable")?,
                    &mime::JSON.to_string()
                );
                let mut mails: Vec<MailSummary> = mails
                    .iter()
                    .map(|mail| {
                        serde_json::from_value::<MailSummary>(mail.summary())
                            .map_err(|e| format!("{:?}", e))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                mails.sort_by(|a, b| a.id.cmp(&b.id));
                let txt: String = response.body_string().await?;
                let mut txt: Vec<MailSummary> = serde_json::from_str(&txt)?;
                txt.sort_by(|a, b| a.id.cmp(&b.id));
                assert_eq!(mails, txt);
            }

            // Get one mail
            {
                #[derive(Debug, Serialize, Deserialize, PartialEq)]
                struct MailAll {
                    headers: Vec<String>,
                    raw: Vec<String>,
                    data: String,
                }
                #[allow(clippy::indexing_slicing)]
                let mail: &Mail = &mails[0];

                // Non existent mail id
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mail/1")?);
                let response: Response = app.respond(request).await?;
                assert_eq!(response.status(), StatusCode::NotFound);

                // Valid mail id
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!(
                        "http://localhost/mail/{}",
                        mail.get_id().to_string()
                    ))?,
                );
                let mut response: Response = app.respond(request).await?;
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .ok_or("Content-Type header unavailable")?,
                    &mime::JSON.to_string()
                );
                let mail: MailAll = serde_json::from_value(json!({
                    "headers": mail.get_headers(&HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(&HeaderRepresentation::Raw),
                    "data": mail.get_text().ok_or("data mail empty")?.clone(),
                }))?;
                let txt: MailAll = serde_json::from_str(&response.body_string().await?)?;
                assert_eq!(mail, txt);
            }

            // Faking new mail
            #[cfg(feature = "faking")]
            {
                // Without number of mail
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/fake")?);
                let mut response: Response = app.respond(request).await?;

                let body = response.body_string().await?;
                assert_eq!(body, "OK: 1");

                let fake_mail_1 = rx_mail_from_faking.next().await.ok_or("no mail")?;
                assert!(fake_mail_1
                    .get_text()
                    .ok_or(" no data text")?
                    .starts_with("Lorem ipsum dolor sit "));

                // With 1 mail
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/fake/1")?);
                let mut response: Response = app.respond(request).await?;

                let fake_mail_2 = rx_mail_from_faking.next().await.ok_or("no mail")?;
                assert!(fake_mail_2
                    .get_text()
                    .ok_or("no data text")?
                    .starts_with("Lorem ipsum dolor sit "));

                let body = response.body_string().await?;
                assert_eq!(body, "OK: 1");

                // With 11 mail
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/fake/11")?);
                let mut response: Response = app.respond(request).await?;

                let mut mails = Vec::new();
                for _ in 0..11 {
                    mails.push(rx_mail_from_faking.next().await.ok_or("no mail")?);
                }
                assert_eq!(mails.len(), 11);

                let body = response.body_string().await?;
                assert_eq!(body, "OK: 11");

                // No more waiting in the fake stream
                assert!(rx_mail_from_faking.is_empty());
            }

            Ok(())
        }

        init();

        task::block_on(the_test())
    }
}
