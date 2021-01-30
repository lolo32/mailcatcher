use async_std::channel;
use futures::StreamExt;
use tide::{prelude::json, Body, Request, Response, Server, StatusCode};
use ulid::Ulid;

use crate::{
    http::State,
    mail::{broker::MailEvt, HeaderRepresentation, Mail, Type},
};

/// Append the routes to retrieve the mail list or mail details: `/mails` or `/mail/*`
pub fn append_route<T>(app: &mut Server<State<T>>)
where
    T: Send + Clone + 'static,
{
    // Get all mail list
    let _route_mails = app.at("/mails").get(|req: Request<State<T>>| async move {
        let (s, mut r): crate::Channel<Mail> = channel::unbounded();
        req.state().mail_broker.send(MailEvt::GetAll(s)).await?;

        let mut resp: Vec<serde_json::Value> = Vec::new();
        while let Some(mail) = r.next().await {
            resp.push(mail.summary());
        }

        Body::from_json(&json!(&resp))
    });
    // Get mail details
    let _route_mail_id = app
        .at("/mail/:id")
        .get(|req: Request<State<T>>| async move {
            if let Some(mail) = get_mail(&req).await? {
                let obj: serde_json::Value = json!({
                    "headers": mail.get_headers(&HeaderRepresentation::Humanized),
                    "raw": mail.get_headers(&HeaderRepresentation::Raw),
                    "data": mail.get_text().expect("json mail data").clone(),
                });
                Ok(Body::from_json(&obj).expect("body from json").into())
            } else {
                Ok(Response::new(StatusCode::NotFound))
            }
        });
    // Get mail in text format
    let _route_mail_id_text = app
        .at("/mail/:id/text")
        .get(|req: Request<State<T>>| async move {
            (get_mail(&req).await?).map_or_else(
                || Ok(Response::new(StatusCode::NotFound)),
                |mail| {
                    Ok(Body::from_string(match mail.get_text() {
                        Some(text) => text.to_string(),
                        None => "".to_owned(),
                    })
                    .into())
                },
            )
        });
    // Get mail in html format
    let _route_mail_id_html = app
        .at("/mail/:id/html")
        .get(|req: Request<State<T>>| async move {
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
    let _route_mail_id_source =
        app.at("/mail/:id/source")
            .get(|req: Request<State<T>>| async move {
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
        let mail: Option<Mail> = r.next().await.expect("received mail");
        log::trace!("mail with id {} found {:?}", id, mail);
        mail
    } else {
        log::trace!("mail with id invalid {}", id);
        None
    })
}
