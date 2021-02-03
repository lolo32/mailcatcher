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
    fn test_routes() {
        task::block_on(async {
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
                tx_mail_broker
                    .send(MailEvt::NewMail(mail))
                    .await
                    .expect("sent new mail");
            }
            let _mail_broker_task = spawn_task_and_swallow_log_errors(
                "test_routes_mails".to_owned(),
                mail_broker(rx_mail_broker, "test_routes_broker"),
            );

            // Init the HTTP side
            let params: Params = Params {
                mail_broker: tx_mail_broker,
                rx_mails: rx_new_mail,
                #[cfg(feature = "faking")]
                tx_new_mail: tx_mail_from_faking,
            };
            let app: Server<State<SseEvt>> = init(params).await.expect("tide initialised");

            // Assets
            for (filename, mime_type) in vec![
                ("home.html", mime::HTML),
                ("w3.css", mime::CSS),
                ("hyperapp.js", mime::JAVASCRIPT),
            ] {
                let mut url = Url::parse("http://localhost/").expect("url parse");
                if filename != "home.html" {
                    url.set_path(filename);
                };
                let request: Request = Request::new(Method::Get, url);
                let mut response: Response = app.respond(request).await.expect("request responded");
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .expect("Content-Type exists"),
                    &mime_type.to_string()
                );
                let mut fs: File = File::open(get_asset_path().join(filename))
                    .await
                    .expect("opening asset file");
                let mut home_content: String = String::new();
                let read: usize = fs
                    .read_to_string(&mut home_content)
                    .await
                    .expect("file read");
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                assert_eq!(
                    response
                        .body_string()
                        .await
                        .expect("extract body from response"),
                    home_content
                );
            }

            // Test deflate
            {
                let mut request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/").expect("url parsing"),
                );
                let _ = request.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
                let mut response: Response = app.respond(request).await.expect("received response");
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .expect("Content-Type header present"),
                    &mime::HTML.to_string()
                );
                assert_eq!(
                    response
                        .header(headers::CONTENT_ENCODING)
                        .expect("Content-Encoding header present"),
                    "deflate"
                );
                let mut fs: File = File::open(get_asset_path().join("home.html"))
                    .await
                    .expect("asset/home.html opened");
                let mut home_content: String = String::new();
                let read: usize = fs
                    .read_to_string(&mut home_content)
                    .await
                    .expect("home.html read");
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                let res_content: Vec<u8> = response.body_bytes().await.expect("response body got");
                // Deflate the content
                let res_content: Vec<u8> = miniz_oxide::inflate::decompress_to_vec(&res_content)
                    .expect("body uncompressed");
                let res_content: String =
                    String::from_utf8(res_content).expect("string to be utf-8 valid");
                assert_eq!(res_content, home_content);
            }

            // Get all mails
            {
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/mails").expect("url parse"),
                );
                let mut response: Response = app.respond(request).await.expect("received response");
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .expect("Content-Type header present"),
                    &mime::JSON.to_string()
                );
                let mut mails: Vec<MailSummary> = mails
                    .iter()
                    .map(|mail| {
                        serde_json::from_value::<MailSummary>(mail.summary())
                            .expect("convert from Value")
                    })
                    .collect();
                mails.sort_by(|a, b| a.id.cmp(&b.id));
                let txt: String = response.body_string().await.expect("response body got");
                let mut txt: Vec<MailSummary> =
                    serde_json::from_str(&txt).expect("convertion from JSON");
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
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/mail/1").expect("url parsing"),
                );
                let response: Response = app.respond(request).await.expect("response received");
                assert_eq!(response.status(), StatusCode::NotFound);

                // Valid mail id
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!(
                        "http://localhost/mail/{}",
                        mail.get_id().to_string()
                    ))
                    .expect("url parsing"),
                );
                let mut response: Response = app.respond(request).await.expect("received response");
                assert_eq!(
                    response
                        .header(headers::CONTENT_TYPE)
                        .expect("Content-Type header present"),
                    &mime::JSON.to_string()
                );
                let mail: MailAll = serde_json::from_value(json!({
                    "headers": mail.get_headers(&HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(&HeaderRepresentation::Raw),
                    "data": mail.get_text().expect("mail data").clone(),
                }))
                .expect("convert from JSON to MailAll");
                let txt: MailAll =
                    serde_json::from_str(&response.body_string().await.expect("response body"))
                        .expect("JSON to MailAll");
                assert_eq!(mail, txt);
            }

            // Faking new mail
            #[cfg(feature = "faking")]
            {
                // Without number of mail
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/fake").expect("url parse"),
                );
                let mut response: Response = app.respond(request).await.expect("received response");

                let body = response.body_string().await.expect("body string");
                assert_eq!(body, "OK: 1");

                let fake_mail_1 = rx_mail_from_faking.next().await.expect("mail");
                assert!(fake_mail_1
                    .get_text()
                    .expect("text")
                    .starts_with("Lorem ipsum dolor sit "));

                // With 1 mail
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/fake/1").expect("url parse"),
                );
                let mut response: Response = app.respond(request).await.expect("received response");

                let fake_mail_2 = rx_mail_from_faking.next().await.expect("mail");
                assert!(fake_mail_2
                    .get_text()
                    .expect("text")
                    .starts_with("Lorem ipsum dolor sit "));

                let body = response.body_string().await.expect("body string");
                assert_eq!(body, "OK: 1");

                // With 11 mail
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse("http://localhost/fake/11").expect("url parse"),
                );
                let mut response: Response = app.respond(request).await.expect("received response");

                let mut mails = Vec::new();
                for _ in 0..11 {
                    mails.push(rx_mail_from_faking.next().await.expect("mail"));
                }
                assert_eq!(mails.len(), 11);

                let body = response.body_string().await.expect("body string");
                assert_eq!(body, "OK: 11");

                // No more waiting in the fake stream
                assert!(rx_mail_from_faking.is_empty());
            }
        });
    }
}
