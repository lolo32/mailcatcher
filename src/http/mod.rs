use std::{borrow::Cow, time::Duration};

use async_std::{
    channel::{self, Receiver, Sender},
    net::{SocketAddr, ToSocketAddrs},
    task,
};
use broadcaster::BroadcastChannel;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    StreamExt,
};
use log::{debug, error, info, trace};
use tide::{
    http::{headers, mime, Mime},
    prelude::{json, Listener},
    Body, Request, Response, ResponseBuilder, Server, StatusCode,
};
use ulid::Ulid;

use crate::{
    http::{asset::Asset, sse_evt::SseEvt},
    mail::{broker::MailEvt, HeaderRepresentation, Mail, Type},
    utils::spawn_task_and_swallow_log_errors,
};

mod asset;
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

pub async fn serve_http(params: Params) -> crate::Result<Server<State<SseEvt>>> {
    // Stream reader and writer for SSE notifications
    let sse_stream = BroadcastChannel::new();

    let sse_stream_new_mail = sse_stream.clone();
    let mut rx_mails: Receiver<Mail> = params.rx_mails;
    let _ = spawn_task_and_swallow_log_errors("Task: Mail notifier".into(), async move {
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
    let _ = spawn_task_and_swallow_log_errors("Task: Noop stream emptier".into(), async move {
        loop {
            trace!("Consume SSE notification stream");
            // Do nothing, it's just to empty the stream
            let _ = sse_noop_stream.next().await;
        }
    });

    // Task sending ping to SSE terminators
    let sse_tx_ping_stream = sse_stream.clone();
    let _ = spawn_task_and_swallow_log_errors("Task: Ping SSE sender".into(), async move {
        loop {
            trace!("Sending ping");
            // Do nothing, it's just to empty the stream
            sse_tx_ping_stream.send(&SseEvt::Ping).await.unwrap();
            task::sleep(Duration::from_secs(10)).await;
        }
    });

    let state: State<SseEvt> = State {
        sse_stream,
        mail_broker: params.mail_broker,
        #[cfg(feature = "faking")]
        new_fake_mail: params.tx_new_mail,
    };
    let mut app: Server<State<SseEvt>> = tide::with_state(state);

    let _ = app
        .at("/")
        .get(|req: Request<State<SseEvt>>| async { Ok(send_asset(req, "home.html", mime::HTML)) });
    let _ = app
        .at("/hyperapp.js")
        .get(|req: Request<State<SseEvt>>| async {
            Ok(send_asset(req, "hyperapp.js", mime::JAVASCRIPT))
        });
    let _ = app
        .at("/w3.css")
        .get(|req: Request<State<SseEvt>>| async { Ok(send_asset(req, "w3.css", mime::CSS)) });

    // Get all mail list
    let _ = app
        .at("/mails")
        .get(|req: Request<State<SseEvt>>| async move {
            let (s, mut r): crate::Channel<Mail> = channel::unbounded();
            req.state().mail_broker.send(MailEvt::GetAll(s)).await?;

            let mut resp: Vec<serde_json::Value> = Vec::new();
            while let Some(mail) = r.next().await {
                resp.push(mail.summary());
            }

            Body::from_json(&json!(&resp))
        });
    // Get mail details
    let _ = app
        .at("/mail/:id")
        .get(|req: Request<State<SseEvt>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                let obj: serde_json::Value = json!({
                    "headers": mail.get_headers(HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(HeaderRepresentation::Raw),
                    "data": mail.get_text().unwrap().clone(),
                });
                Ok(Body::from_json(&obj).unwrap().into())
            } else {
                Ok(Response::new(StatusCode::NotFound))
            }
        });
    // Get mail in text format
    let _ = app
        .at("/mail/:id/text")
        .get(|req: Request<State<SseEvt>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                Ok(Body::from_string(match mail.get_text() {
                    Some(text) => text.to_string(),
                    None => "".to_string(),
                })
                .into())
            } else {
                Ok(Response::new(StatusCode::NotFound))
            }
        });
    // Get mail in html format
    let _ = app
        .at("/mail/:id/html")
        .get(|req: Request<State<SseEvt>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                let s: &str = match mail.get_html() {
                    Some(text) => &text[..],
                    None => "",
                };
                Ok(Body::from_bytes(s.as_bytes().to_vec()).into())
            } else {
                Ok(Response::new(StatusCode::NotFound))
            }
        });
    // Get RAW format mail
    let _ = app
        .at("/mail/:id/source")
        .get(|req: Request<State<SseEvt>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                if let Some(raw) = mail.get_data(&Type::Raw) {
                    let (headers, body): (String, String) = Mail::split_header_body(raw);
                    return Ok(Body::from_json(&json!({
                        "headers": headers,
                        "content": body,
                    }))?
                    .into());
                }
            }
            Ok(Response::new(StatusCode::NotFound))
        });

    // Remove all mails
    let _ = app
        .at("/remove/all")
        .get(|req: Request<State<SseEvt>>| async move {
            let (s, mut r): crate::Channel<Ulid> = channel::unbounded();
            req.state().mail_broker.send(MailEvt::RemoveAll(s)).await?;
            let mut nb: usize = 0;
            while let Some(id) = r.next().await {
                nb += 1;
                match req.state().sse_stream.send(&SseEvt::DelMail(id)).await {
                    Ok(()) => trace!("Success notification of removal: {}", id.to_string()),
                    Err(e) => error!("Notification of removal {}: {:?}", id.to_string(), e),
                }
            }
            Ok(format!("OK: {}", nb))
        });
    // Remove a mail by id
    let _ = app
        .at("/remove/:id")
        .get(|req: Request<State<SseEvt>>| async move {
            let id: &str = req.param("id")?;
            if let Ok(id) = Ulid::from_string(id) {
                let (s, mut r): crate::Channel<Option<Ulid>> = channel::bounded(1);
                req.state().mail_broker.send(MailEvt::Remove(s, id)).await?;
                let mail: Option<Ulid> = r.next().await.unwrap();
                if mail.is_some() {
                    info!("mail removed {:?}", mail);
                    req.state().sse_stream.send(&SseEvt::DelMail(id)).await?;
                    return Ok("OK: 1".into());
                }
            }
            Ok(Response::new(StatusCode::NotFound))
        });

    // SSE stream
    let _ = app.at("/sse").get(tide::sse::endpoint(sse::handle_sse));

    #[cfg(feature = "faking")]
    {
        async fn faking(req: Request<State<SseEvt>>) -> tide::Result<String> {
            let nb: usize = req
                .param("nb")
                .map(|nb| usize::from_str_radix(nb, 10).unwrap_or(1))
                .unwrap_or(1);

            for _ in 0..nb {
                let mail: Mail = Mail::fake();

                match req.state().new_fake_mail.send(mail).await {
                    Ok(()) => debug!("New faked mail sent!"),
                    Err(e) => debug!("New mail error: {:?}", e),
                }
            }

            Ok(format!("OK: {}", nb))
        }

        // Generate one fake mail
        let _ = app.at("/fake").get(faking);
        // Generate :nb fake mails, 1 if not a number
        let _ = app.at("/fake/:nb").get(faking);
    }

    Ok(app)
}

pub async fn bind_http<T>(app: Server<State<T>>, port: u16) -> crate::Result<()>
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
    for info in listener.info().iter() {
        info!("HTTP listening on {}", info);
    }
    // Accept connections
    listener.accept().await?;

    unreachable!()
}

/// Retrieve a mail from the the request, extracting the ID
async fn get_mail<T>(req: &Request<State<T>>) -> tide::Result<Option<Mail>>
where
    T: Send + Clone + 'static,
{
    // Extract the ID
    let id: &str = req.param("id")?;
    // Convert ID string to Ulid
    Ok(if let Ok(id) = Ulid::from_string(id) {
        let (s, mut r): crate::Channel<Option<Mail>> = channel::bounded(1);
        req.state()
            .mail_broker
            .send(MailEvt::GetMail(s, id))
            .await?;
        // Get mails pool
        let mail: Option<Mail> = r.next().await.unwrap();
        trace!("mail with id {} found {:?}", id, mail);
        mail
    } else {
        trace!("mail with id invalid {}", id);
        None
    })
}

/// Generate a Response based on the name of the asset and the mime type
fn send_asset<T>(req: Request<State<T>>, name: &str, mime: Mime) -> Response
where
    T: Send + Clone + 'static,
{
    // Look if the response can be compressed in deflate, or not
    let compressed: bool = if let Some(header_value) = req.header(headers::ACCEPT_ENCODING) {
        header_value.get(0).unwrap().as_str().contains("deflate")
    } else {
        false
    };
    // Retrieve the asset, either integrated during release compilation, or read from filesystem if it's debug build
    let content: Cow<[u8]> = Asset::get(name).unwrap();
    // If compression if available, ...
    let content: Cow<[u8]> = if compressed {
        // ... do nothing
        content
    } else {
        // ... uncompress the file content
        let uncompressed: Vec<u8> = miniz_oxide::inflate::decompress_to_vec(&content[..]).unwrap();
        Cow::Owned(uncompressed)
    };
    debug!("content_len: {:?}", content.len());

    // Build the Response
    let response: ResponseBuilder = Response::builder(StatusCode::Ok)
        // specify the mime type
        .content_type(mime)
        // then the file length
        .header(headers::CONTENT_LENGTH, content.len().to_string());
    // If compression enabled, add the header to response
    let response: ResponseBuilder = if compressed {
        debug! {"using deflate compression output"};
        response.header(headers::CONTENT_ENCODING, "deflate")
    } else {
        response
    };
    // If the last modified date is available, add the content to the header
    let response: ResponseBuilder = if let Some(modif) = Asset::modif(name) {
        response.header(headers::LAST_MODIFIED, modif)
    } else {
        response
    };

    // Return the Response with content
    response.body(&*content).build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::broker::mail_broker;
    use async_std::{fs::File, path::Path};
    use futures::AsyncReadExt;
    use tide::http::{Method, Request, Response, Url};
    use tide::prelude::{Deserialize, Serialize};

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
            let _ = spawn_task_and_swallow_log_errors(
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
            let app: Server<State<SseEvt>> = serve_http(params).await.unwrap();

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
                let req: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!("http://localhost/{}", url)).unwrap(),
                );
                let mut res: Response = app.respond(req).await.unwrap();
                assert_eq!(
                    res.header(headers::CONTENT_TYPE).unwrap(),
                    &mime_type.to_string()
                );
                let mut fs: File = File::open(Path::new("asset").join(filename)).await.unwrap();
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await.unwrap();
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                assert_eq!(res.body_string().await.unwrap(), home_content);
            }

            // Test deflate
            {
                let mut req: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/").unwrap());
                let _ = req.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
                let mut res: Response = app.respond(req).await.unwrap();
                assert_eq!(
                    res.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::HTML.to_string()
                );
                assert_eq!(res.header(headers::CONTENT_ENCODING).unwrap(), "deflate");
                let mut fs: File = File::open(Path::new("asset").join("home.html"))
                    .await
                    .unwrap();
                let mut home_content: String = String::new();
                let read: usize = fs.read_to_string(&mut home_content).await.unwrap();
                assert!(read > 0, "File content must have some bytes");
                assert_eq!(read, home_content.len());
                let res_content: Vec<u8> = res.body_bytes().await.unwrap();
                // Deflate the content
                let res_content: Vec<u8> =
                    miniz_oxide::inflate::decompress_to_vec(&res_content).unwrap();
                let res_content: String = String::from_utf8(res_content).unwrap();
                assert_eq!(res_content, home_content);
            }

            // Get all mails
            {
                let req: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mails").unwrap());
                let mut res: Response = app.respond(req).await.unwrap();
                assert_eq!(
                    res.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::JSON.to_string()
                );
                let mut mails: Vec<MailSummary> = mails
                    .iter()
                    .map(|mail| serde_json::from_value::<MailSummary>(mail.summary()).unwrap())
                    .collect();
                mails.sort_by(|a, b| a.id.cmp(&b.id));
                let txt: String = res.body_string().await.unwrap();
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
                let req: Request =
                    Request::new(Method::Get, Url::parse("http://localhost/mail/1").unwrap());
                let res: Response = app.respond(req).await.unwrap();
                assert_eq!(res.status(), StatusCode::NotFound);

                // Valid mail id
                let req: Request = Request::new(
                    Method::Get,
                    Url::parse(&format!(
                        "http://localhost/mail/{}",
                        mail.get_id().to_string()
                    ))
                    .unwrap(),
                );
                let mut res: Response = app.respond(req).await.unwrap();
                assert_eq!(
                    res.header(headers::CONTENT_TYPE).unwrap(),
                    &mime::JSON.to_string()
                );
                let mail: MailAll = serde_json::from_value(json!({
                    "headers": mail.get_headers(HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(HeaderRepresentation::Raw),
                    "data": mail.get_text().unwrap().clone(),
                }))
                .unwrap();
                let txt: MailAll = serde_json::from_str(&res.body_string().await.unwrap()).unwrap();
                assert_eq!(mail, txt);
            }
        });
    }
}
