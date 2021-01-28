use async_std::channel;
use futures::StreamExt;
use log::{error, info, trace};
use serde_json::json;
use tide::{
    http::Response,
    Body, Request, Server, StatusCode,
};
use ulid::Ulid;

use utils::get_mail;

use crate::mail::{broker::MailEvt, HeaderRepresentation, Mail, Type};

use super::{sse, sse_evt::SseEvt, State};

#[cfg(feature = "faking")]
mod faking;
mod static_;
mod utils;

#[allow(clippy::too_many_lines)]
pub async fn init(state: State<SseEvt>) -> crate::Result<Server<State<SseEvt>>> {
    let mut app: Server<State<SseEvt>> = tide::with_state(state);

    static_::append_route(&mut app).await;

    // Get all mail list
    let _route = app
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
    let _route = app
        .at("/mail/:id")
        .get(|req: Request<State<SseEvt>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                let obj: serde_json::Value = json!({
                    "headers": mail.get_headers(&HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(&HeaderRepresentation::Raw),
                    "data": mail.get_text().unwrap().clone(),
                });
                Ok(Body::from_json(&obj).unwrap().into())
            } else {
                Ok(Response::new(StatusCode::NotFound))
            }
        });
    // Get mail in text format
    let _route = app
        .at("/mail/:id/text")
        .get(|req: Request<State<SseEvt>>| async move {
            (get_mail(&req).await?).map_or_else(
                || Ok(Response::new(StatusCode::NotFound)),
                |mail| {
                    Ok(Body::from_string(match mail.get_text() {
                        Some(text) => text.to_string(),
                        None => "".to_string(),
                    })
                    .into())
                },
            )
        });
    // Get mail in html format
    let _route = app
        .at("/mail/:id/html")
        .get(|req: Request<State<SseEvt>>| async move {
            (get_mail(&req).await?).map_or_else(
                || Ok(Response::new(StatusCode::NotFound)),
                |mail| {
                    let s = match mail.get_html() {
                        Some(text) => &text[..],
                        None => "",
                    };
                    Ok(Body::from_bytes(s.as_bytes().to_vec()).into())
                },
            )
        });
    // Get RAW format mail
    let _route = app
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
    let _route = app
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
    let _route = app
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
    let _route = app.at("/sse").get(tide::sse::endpoint(sse::handle));

    #[cfg(feature = "faking")]
    faking::append_route(&mut app);

    Ok(app)
}
