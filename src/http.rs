use async_channel::Receiver;
use async_std::{
    prelude::*,
    sync::{Arc, Mutex},
    task,
};
use broadcaster::BroadcastChannel;
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use tide::{prelude::*, sse, Body, Request, Response, StatusCode};
use ulid::Ulid;

use crate::encoding::decode_string;
use crate::{
    mail::{Mail, Type},
    Result,
};

#[derive(Clone)]
struct State {
    chan_sse: BroadcastChannel<Mail, UnboundedSender<Mail>, UnboundedReceiver<Mail>>,
}

lazy_static! {
    static ref MAILS: Arc<Mutex<fnv::FnvHashMap<Ulid, Mail>>> = Default::default();
}

pub async fn serve_http(
    port: u16,
    server_name: String,
    mut rx_mails: Receiver<Mail>,
) -> Result<()> {
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
            mails.lock().await.insert(mail.get_id(), mail);
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
    // Get all mail list
    app.at("/mails").get(|_req| async move {
        // let m = mails(Ulid::nil(), None);
        let mails = Arc::clone(&MAILS);
        let resp: Vec<_> = { mails.lock().await.values().map(MailShort::new).collect() };

        Body::from_json(&resp)
    });
    // Get mail details
    app.at("/mail/:id").get(|req: Request<State>| async move {
        if let Some(mail) = get_mail(&req).await? {
            let obj = json! ({
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
        {
            mails.lock().await.clear();
        }
        Ok("OK")
    });
    app.at("/remove/:id").get(|req: Request<State>| async move {
        let id = req.param("id")?;
        if let Ok(id) = Ulid::from_string(id) {
            let mails = Arc::clone(&MAILS);
            let mail = { mails.lock().await.remove(&id) };
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

    app.listen(format!("localhost:{}", port)).await?;
    Ok(())
}

async fn get_mail(req: &Request<State>) -> tide::Result<Option<Mail>> {
    let id = req.param("id")?;
    if let Ok(id) = Ulid::from_string(id) {
        let mails = Arc::clone(&MAILS);
        let mail = { mails.lock().await.get(&id).cloned() };
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
