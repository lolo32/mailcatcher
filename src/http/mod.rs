use std::time::Duration;

use async_channel::Receiver;
use async_std::{
    net::ToSocketAddrs,
    sync::{Arc, RwLock},
    task,
};
use broadcaster::BroadcastChannel;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    StreamExt,
};
use regex::Regex;
use tide::{
    http::{headers, mime},
    prelude::*,
    Body, Request, Response, StatusCode,
};
use ulid::Ulid;

use asset::Asset;

use crate::encoding::decode_string;
use crate::http::sse_evt::SseEvt;
use crate::mail::{Mail, Type};
use crate::utils::spawn_task_and_swallow_log_errors;

mod asset;
mod sse;
mod sse_evt;

#[derive(Clone)]
pub struct State {
    sse_stream: BroadcastChannel<SseEvt, UnboundedSender<SseEvt>, UnboundedReceiver<SseEvt>>,
}

lazy_static! {
    static ref MAILS: Arc<RwLock<fnv::FnvHashMap<Ulid, Mail>>> = Default::default();
}

pub async fn serve_http(port: u16, mut rx_mails: Receiver<Mail>) -> crate::Result<()> {
    let sse_stream = BroadcastChannel::new();

    // Process mails that are received by the SMTP side
    let sse_tx_stream = sse_stream.clone();
    spawn_task_and_swallow_log_errors("Task: Mail notifier".into(), async move {
        let mails = Arc::clone(&MAILS);
        while let Some(mail) = rx_mails.next().await {
            info!(">>> Received new mail: {:?}", mail);
            // Notify javascript side by SSE
            match sse_tx_stream.send(&SseEvt::NewMail(mail.clone())).await {
                Ok(_) => trace!(">>> New mail notification sent to channel"),
                Err(e) => trace!(">>> Err new mail notification to channel: {:?}", e),
            }
            mails.write().await.insert(mail.get_id(), mail);
        }
        Ok(())
    });

    // Noop consumer to empty the ctream
    let mut sse_noop_stream = sse_stream.clone();
    spawn_task_and_swallow_log_errors("Task: Noop stream emptier".into(), async move {
        loop {
            // Do nothing, it's just to empty the stream
            sse_noop_stream.next().await;
        }
    });

    // Noop consumer to empty the ctream
    let sse_tx_ping_stream = sse_stream.clone();
    spawn_task_and_swallow_log_errors("Task: Ping SSE sender".into(), async move {
        loop {
            trace!("Sending ping");
            // Do nothing, it's just to empty the stream
            sse_tx_ping_stream.send(&SseEvt::Ping).await.unwrap();
            task::sleep(Duration::from_secs(10)).await;
        }
    });

    let state = State { sse_stream };
    let mut app = tide::with_state(state);

    app.at("/").get(|_req| async {
        const HOME: &str = "home.html";

        let home = Asset::get(HOME).unwrap();

        let response = Response::builder(StatusCode::Ok)
            .content_type(mime::HTML)
            .header(headers::CONTENT_LENGTH, home.len().to_string());
        let response = if let Some(modif) = Asset::modif(HOME) {
            response.header(headers::LAST_MODIFIED, modif)
        } else {
            response
        };
        Ok(response.body(&*home).build())
    });
    app.at("/hyperapp.js").get(|_req| async {
        const HYPERAPP: &str = "hyperapp.js";
        let hyperapp = Asset::get(HYPERAPP).unwrap();

        let response = Response::builder(StatusCode::Ok)
            .content_type(mime::JAVASCRIPT)
            .header(headers::CONTENT_LENGTH, hyperapp.len().to_string());
        let response = if let Some(modif) = Asset::modif(HYPERAPP) {
            response.header(headers::LAST_MODIFIED, modif)
        } else {
            response
        };
        Ok(response.body(&*hyperapp).build())
    });
    app.at("/w3.css").get(|_req| async {
        const W3: &str = "w3.css";
        let w3 = Asset::get(W3).unwrap();

        let response = Response::builder(StatusCode::Ok)
            .content_type(mime::CSS)
            .header(headers::CONTENT_LENGTH, w3.len().to_string());
        let response = if let Some(modif) = Asset::modif(W3) {
            response.header(headers::LAST_MODIFIED, modif)
        } else {
            response
        };
        Ok(response.body(&*w3).build())
    });
    // Get all mail list
    app.at("/mails").get(|_req| async move {
        let mails = Arc::clone(&MAILS);
        let resp: Vec<_> = { mails.read().await.values() }
            .map(MailShort::new)
            .collect();

        Body::from_json(&resp)
    });
    // Get mail details
    app.at("/mail/:id").get(|req: Request<State>| async move {
        if let Some(mail) = get_mail(&req).await? {
            let obj = json!({
                "headers": mail
                    .get_headers()
                    .iter()
                    .map(|header| decode_string(header).to_string())
                    .collect::<Vec<_>>(),
                "raw": mail.get_headers().clone(),
                "data": mail.get_text().unwrap().clone(),
            });
            return Ok(Body::from_json(&obj).unwrap().into());
        }
        Ok(Response::new(StatusCode::NotFound))
    });
    app.at("/mail/:id/text")
        .get(|req: Request<State>| async move {
            if let Some(mail) = get_mail(&req).await? {
                return Ok(Body::from_string(match mail.get_text() {
                    Some(text) => text.to_string(),
                    None => "".to_string(),
                })
                .into());
            }
            Ok(Response::new(StatusCode::NotFound))
        });
    app.at("/mail/:id/html")
        .get(|req: Request<State>| async move {
            if let Some(mail) = get_mail(&req).await? {
                let s = match mail.get_html() {
                    Some(text) => &text[..],
                    None => "",
                };
                return Ok(Body::from_bytes(s.as_bytes().to_vec()).into());
            }
            Ok(Response::new(StatusCode::NotFound))
        });
    app.at("/mail/:id/source")
        .get(|req: Request<State>| async move {
            if let Some(mail) = get_mail(&req).await? {
                if let Some(raw) = mail.get_data(&Type::Raw) {
                    let reg = Regex::new(
                        "(?P<headers>(?:[^\r\n]+\r?\n)*[^\r\n]+)\r?\n\r?\n(?P<content>.*)",
                    )?;
                    let caps = reg.captures(raw).unwrap();
                    let (headers, content) = (caps.name("headers"), caps.name("content"));
                    return Ok(Body::from_json(&json!({
                        "headers": headers.unwrap().as_str(),
                        "content": content.unwrap().as_str(),
                    }))?
                    .into());
                }
            }
            Ok(Response::new(StatusCode::NotFound))
        });

    // Remove
    app.at("/remove/all").get(|_req| async move {
        let mails = Arc::clone(&MAILS);
        mails.write().await.clear();
        Ok("OK")
    });
    app.at("/remove/:id").get(|req: Request<State>| async move {
        let id = req.param("id")?;
        if let Ok(id) = Ulid::from_string(id) {
            let mails = Arc::clone(&MAILS);
            let mail = mails.write().await.remove(&id);
            if mail.is_some() {
                info!("mail removed {:?}", mail);
                req.state().sse_stream.send(&SseEvt::DelMail(id)).await?;
                return Ok("OK".into());
            }
        }
        Ok(Response::new(StatusCode::NotFound))
    });

    // SSE stream
    app.at("/sse").get(tide::sse::endpoint(sse::handle_sse));

    let mut listener = app
        .bind(
            format!("localhost:{}", port)
                .to_socket_addrs()
                .await?
                .collect::<Vec<_>>(),
        )
        .await?;
    for info in listener.info().iter() {
        info!("HTTP listening on {}", info);
    }
    listener.accept().await?;
    Ok(())
}

async fn get_mail<'a>(req: &Request<State>) -> tide::Result<Option<Mail>> {
    let id = req.param("id")?;
    if let Ok(id) = Ulid::from_string(id) {
        let mails = Arc::clone(&MAILS);
        let mail = mails.read().await.get(&id).cloned();
        trace!("mail with id {} found {:?}", id, mail);
        return Ok(mail);
    }
    trace!("mail with id not found {}", id);
    Ok(None)
}

#[derive(Debug, Serialize)]
struct MailShort {
    id: String,
    from: String,
    to: Vec<String>,
    subject: String,
    date: i64,
    size: usize,
}

impl MailShort {
    pub fn new(mail: &Mail) -> Self {
        Self {
            id: mail.get_id().to_string(),
            from: mail.from().to_string(),
            to: mail.to().iter().map(|s| s.to_string()).collect(),
            subject: mail.get_subject().to_string(),
            date: mail.get_date().timestamp(),
            size: mail.get_size(),
        }
    }
}
