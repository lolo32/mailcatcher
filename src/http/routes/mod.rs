use async_std::channel;
use futures::StreamExt;
use log::{error, info, trace};
use serde_json::json;
use tide::{
    http::{mime, Response},
    Body, Request, Server, StatusCode,
};
use ulid::Ulid;

use utils::get_mail;

use crate::{
    http::asset::send as send_asset,
    mail::{broker::MailEvt, HeaderRepresentation, Mail, Type},
};

use super::{sse, sse_evt::SseEvt, State};

mod utils;

pub async fn init(state: State<SseEvt>) -> crate::Result<Server<State<SseEvt>>> {
    let mut app: Server<State<SseEvt>> = tide::with_state(state);

    let _ = app.at("/").get(|req: Request<State<SseEvt>>| async move {
        Ok(send_asset(&req, "home.html", mime::HTML))
    });
    let _ = app
        .at("/hyperapp.js")
        .get(|req: Request<State<SseEvt>>| async move {
            Ok(send_asset(&req, "hyperapp.js", mime::JAVASCRIPT))
        });
    let _ = app
        .at("/w3.css")
        .get(
            |req: Request<State<SseEvt>>| async move { Ok(send_asset(&req, "w3.css", mime::CSS)) },
        );

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
    let _ = app
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
    let _ = app
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
    let _ = app.at("/sse").get(tide::sse::endpoint(sse::handle));

    #[cfg(feature = "faking")]
    {
        use log::debug;

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
