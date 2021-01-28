use async_std::channel;
use futures::StreamExt;
use log::{error,trace,info};
use tide::{Request, Response, Server, StatusCode};
use ulid::Ulid;

use crate::{http::{sse_evt::SseEvt,State},mail::broker::MailEvt};

pub fn append_route(app: &mut Server<State<SseEvt>>){
    // Remove all mails
    let _route_remove_all = app.at("/remove/all").get(|req: Request<State<SseEvt>>| async move {
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
    let _route_remove_id = app.at("/remove/:id").get(|req: Request<State<SseEvt>>| async move {
        let id: &str = req.param("id")?;
        if let Ok(id) = Ulid::from_string(id) {
            let (s, mut r): crate::Channel<Option<Ulid>> = channel::bounded(1);
            req.state().mail_broker.send(MailEvt::Remove(s, id)).await?;
            let mail: Option<Ulid> = r.next().await.expect("received mail id");
            if mail.is_some() {
                info!("mail removed {:?}", mail);
                req.state().sse_stream.send(&SseEvt::DelMail(id)).await?;
                return Ok("OK: 1".into());
            }
        }
        Ok(Response::new(StatusCode::NotFound))
    });
}
