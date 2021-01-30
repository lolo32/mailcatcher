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
use log::{error, info, trace};
use tide::{prelude::Listener, Server};

use crate::{
    http::sse_evt::SseEvt,
    mail::{broker::MailEvt, Mail},
    utils::spawn_task_and_swallow_log_errors,
};

mod asset;
mod routes;
mod sse;
pub mod sse_evt;

#[derive(Clone)]
pub struct State<T>
where
    T: Send + Clone + 'static,
{
    sse_stream: BroadcastChannel<T, UnboundedSender<T>, UnboundedReceiver<T>>,
    mail_broker: Sender<MailEvt>,
    #[cfg(feature = "faking")]
    new_fake_mail: Sender<Mail>,
}

pub struct Params {
    pub mail_broker: Sender<MailEvt>,
    pub rx_mails: Receiver<Mail>,
    #[cfg(feature = "faking")]
    pub tx_new_mail: Sender<Mail>,
}

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
                    info!(">>> Received new mail: {:?}", mail);
                    // Append the mail to the list
                    match sse_stream_new_mail.send(&SseEvt::NewMail(mail)).await {
                        Ok(()) => trace!(">>> New mail notification sent to channel"),
                        Err(e) => error!(">>> Err new mail notification to channel: {:?}", e),
                    }
                }
            }
        });

    // Noop consumer to empty the ctream
    let mut sse_noop_stream = sse_stream.clone();
    let _noop_task =
        spawn_task_and_swallow_log_errors("Task: Noop stream emptier".into(), async move {
            loop {
                trace!("Consume SSE notification stream");
                // Do nothing, it's just to empty the stream
                let _sse_evt = sse_noop_stream.next().await;
            }
        });

    // Task sending ping to SSE terminators
    let sse_tx_ping_stream = sse_stream.clone();
    let _sse_ping_task =
        spawn_task_and_swallow_log_errors("Task: Ping SSE sender".into(), async move {
            loop {
                trace!("Sending ping");
                // Do nothing, it's just to empty the stream
                sse_tx_ping_stream
                    .send(&SseEvt::Ping)
                    .await
                    .expect("sending ping");
                task::sleep(Duration::from_secs(10)).await;
            }
        });

    let state: State<SseEvt> = State {
        sse_stream,
        mail_broker: params.mail_broker,
        #[cfg(feature = "faking")]
        new_fake_mail: params.tx_new_mail,
    };

    Ok(routes::init(state).await?)
}

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
        info!("HTTP listening on {}", info);
    }
    // Accept connections
    listener.accept().await?;

    unreachable!()
}

#[cfg(test)]
mod tests {
    use async_std::{channel, fs::File, path::Path};
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

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_routes() {
        task::block_on(async {
            let (tx_mail_broker, rx_mail_broker): crate::Channel<MailEvt> = channel::unbounded();
            let (_tx_new_mail, rx_new_mail): crate::Channel<Mail> = channel::bounded(1);
            #[cfg(feature = "faking")]
            let (tx_mail_from_smtp, _rx_mail_from_smtp): (
                Sender<Mail>,
                Receiver<Mail>,
            ) = channel::bounded(1);

            // Provide some mails
            let mut mails: Vec<Mail> = Vec::new();
            for _ in 0..10 {
                let mail: Mail = Mail::fake();
                mails.push(mail.clone());
                tx_mail_broker.send(MailEvt::NewMail(mail)).await.unwrap();
            }
            let _mail_broker_task = spawn_task_and_swallow_log_errors(
                "test_routes_mails".to_string(),
                mail_broker(rx_mail_broker),
            );

            // Init the HTTP side
            let params: Params = Params {
                mail_broker: tx_mail_broker,
                rx_mails: rx_new_mail,
                #[cfg(feature = "faking")]
                tx_new_mail: tx_mail_from_smtp,
            };
            let app: Server<State<SseEvt>> = init(params).await.unwrap();

            // Assets
            for (filename, mime_type) in &[
                ("home.html", mime::HTML),
                ("w3.css", mime::CSS),
                ("hyperapp.js", mime::JAVASCRIPT),
            ] {
                let url: &str = if filename == &"home.html" {
                    ""
                } else {
                    filename
                };
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!("http://localhost/{}", url)).unwrap(),
                );
                let mut response: Response = app.respond(request).await.unwrap();
                assert_eq!(
                    response.header(headers::CONTENT_TYPE).unwrap(),
                    &mime_type.to_string()
                );
                let mut fs: File = File::open(Path::new("asset").join(filename)).await.unwrap();
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await.unwrap();
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                assert_eq!(response.body_string().await.unwrap(), home_content);
            }

            // Test deflate
            {
                let mut request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/").unwrap());
                let _ = request.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
                let mut response: Response = app.respond(request).await.unwrap();
                assert_eq!(
                    response.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::HTML.to_string()
                );
                assert_eq!(
                    response.header(headers::CONTENT_ENCODING).unwrap(),
                    "deflate"
                );
                let mut fs: File = File::open(Path::new("asset").join("home.html"))
                    .await
                    .unwrap();
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await.unwrap();
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                let res_content: Vec<u8> = response.body_bytes().await.unwrap();
                // Deflate the content
                let res_content: Vec<u8> =
                    miniz_oxide::inflate::decompress_to_vec(&res_content).unwrap();
                let res_content: String = String::from_utf8(res_content).unwrap();
                assert_eq!(res_content, home_content);
            }

            // Get all mails
            {
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mails").unwrap());
                let mut response: Response = app.respond(request).await.unwrap();
                assert_eq!(
                    response.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::JSON.to_string()
                );
                let mut mails: Vec<MailSummary> = mails
                    .iter()
                    .map(|mail| serde_json::from_value::<MailSummary>(mail.summary()).unwrap())
                    .collect();
                mails.sort_by(|a, b| a.id.cmp(&b.id));
                let txt: String = response.body_string().await.unwrap();
                let mut txt: Vec<MailSummary> = serde_json::from_str(&txt).unwrap();
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
                let mail: &Mail = &mails[0];

                // Non existent mail id
                let request: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mail/1").unwrap());
                let response: Response = app.respond(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::NotFound);

                // Valid mail id
                let request: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!(
                        "http://localhost/mail/{}",
                        mail.get_id().to_string()
                    ))
                    .unwrap(),
                );
                let mut response: Response = app.respond(request).await.unwrap();
                assert_eq!(
                    response.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::JSON.to_string()
                );
                let mail: MailAll = serde_json::from_value(json!({
                    "headers": mail.get_headers(&HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(&HeaderRepresentation::Raw),
                    "data": mail.get_text().unwrap().clone(),
                }))
                .unwrap();
                let txt: MailAll =
                    serde_json::from_str(&response.body_string().await.unwrap()).unwrap();
                assert_eq!(mail, txt);
            }
        });
    }
}
