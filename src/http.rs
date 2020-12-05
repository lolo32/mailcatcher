use async_channel::Receiver;
use async_std::{
    net::ToSocketAddrs,
    prelude::*,
    sync::{Arc, RwLock},
    task,
};
use broadcaster::BroadcastChannel;
use chrono::NaiveDate;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use tide::{
    http::{headers, mime},
    prelude::*,
    sse, Body, Request, Response, StatusCode,
};
use ulid::Ulid;

use crate::encoding::decode_string;
use crate::mail::{Mail, Type};

#[derive(Clone)]
struct State {
    chan_sse: BroadcastChannel<Mail, UnboundedSender<Mail>, UnboundedReceiver<Mail>>,
}

lazy_static! {
    static ref MAILS: Arc<RwLock<fnv::FnvHashMap<Ulid, Mail>>> = Default::default();
}

pub async fn serve_http(port: u16, mut rx_mails: Receiver<Mail>) -> crate::Result<()> {
    let chan_sse = BroadcastChannel::new();

    // Process mails that are received by the SMTP side
    let chan_sse_tx = chan_sse.clone();
    task::spawn(async move {
        let mails = Arc::clone(&MAILS);
        while let Some(mail) = rx_mails.next().await {
            info!(">>> Received new mail: {:?}", mail);
            // Notify javascript side by SSE
            match chan_sse_tx.send(&mail).await {
                Ok(_) => trace!(">>> New mail notification sent to channel"),
                Err(e) => trace!(">>> Err new mail notification to channel: {:?}", e),
            }
            mails.write().await.insert(mail.get_id(), mail);
        }
    });

    // Noop consumer to empty the ctream
    let mut sse_noop = chan_sse.clone();
    task::spawn(async move {
        loop {
            // Do nothing, it's just to empty the stream
            sse_noop.next().await;
        }
    });

    let state = State { chan_sse };
    let mut app = tide::with_state(state);
    app.at("/").get(|_req| async {
        let fs_path = std::env::current_dir()?.join("asset").join("home.html");
        if let Ok(body) = Body::from_file(fs_path).await {
            Ok(body.into())
        } else {
            Ok(Response::new(StatusCode::NotFound))
        }
    });
    app.at("/hyperapp.js").get(|_req| async {
        let fs_path = std::env::current_dir()?.join("asset").join("hyperapp.js");
        if let Ok(body) = Body::from_file(fs_path).await {
            Ok(body.into())
        } else {
            Ok(Response::new(StatusCode::NotFound))
        }
    });
    app.at("/w3.css").get(|_req| async {
        let w3 = include_str!("../asset/w3.css");
        // Tue, 01 Dec 2020 00:00:00 GMT
        let date = NaiveDate::from_ymd(2020, 12, 1)
            .and_hms(0, 0, 0)
            .format("%a, %d %b %Y %T GMT")
            .to_string();
        let res = Response::builder(StatusCode::Ok)
            .content_type(mime::CSS)
            .header(headers::LAST_MODIFIED, date)
            .header(headers::CONTENT_LENGTH, w3.len().to_string())
            .body(w3)
            .build();
        Ok(res)
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
                    Some(text) => text.clone(),
                    None => "".to_string(),
                })
                .into());
            }
            Ok(Response::new(StatusCode::NotFound))
        });
    app.at("/mail/:id/html")
        .get(|req: Request<State>| async move {
            if let Some(mail) = get_mail(&req).await? {
                return Ok(Body::from_string(match mail.get_html() {
                    Some(text) => text.clone(),
                    None => "".to_string(),
                })
                .into());
            }
            Ok(Response::new(StatusCode::NotFound))
        });
    app.at("/mail/:id/source")
        .get(|req: Request<State>| async move {
            if let Some(mail) = get_mail(&req).await? {
                match mail.get_data(&Type::Raw) {
                    Some(raw) => return Ok(Body::from_string(raw.clone()).into()),
                    None => {}
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
            let mail = { mails.write().await.remove(&id) };
            if mail.is_some() {
                info!("mail removed {:?}", mail);
                return Ok("OK".into());
            }
        }
        Ok(Response::new(StatusCode::NotFound))
    });

    // SSE stream
    app.at("/sse")
        .get(sse::endpoint(|req: Request<State>, sender| async move {
            trace!("### new /sse connexion");
            let mut chan_sse = req.state().chan_sse.clone();
            trace!("### chan_sse: {:?}", chan_sse);

            debug!("@ task id: {:?}", task::current());

            while let Some(mail) = chan_sse.next().await {
                info!("### received new Mail, sending to stream: {:?}", mail);
                debug!("@ task id: {:?}", task::current());

                let mail = MailShort::new(&mail);
                let body = Body::from_json(&mail)
                    .unwrap_or_else(|_| "".into())
                    .into_string()
                    .await?;
                trace!("### data to send: {}", body);
                let sent = sender.send("", body, None).await;
                if sent.is_err() {
                    warn!("### Err, disconnected: {:?}", sent);
                    break;
                } else {
                    trace!("### Server-Sent Events sent");
                }
            }
            info!("### Exit /sse");
            Ok(())
        }));

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

async fn get_mail(req: &Request<State>) -> tide::Result<Option<Mail>> {
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
            from: mail.from().clone(),
            to: mail.to().clone(),
            subject: mail.get_subject().clone(),
            date: mail.get_date().timestamp(),
            size: mail.get_size(),
        }
    }
}
