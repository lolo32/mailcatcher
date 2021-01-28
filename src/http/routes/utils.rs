use async_std::channel;
use futures::StreamExt;
use log::trace;
use tide::Request;
use ulid::Ulid;

use crate::mail::{broker::MailEvt, Mail};

use super::super::State;

/// Retrieve a mail from the the request, extracting the ID
pub async fn get_mail<T>(req: &Request<State<T>>) -> tide::Result<Option<Mail>>
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
        trace!("mail with id {} found {:?}", id, mail);
        mail
    } else {
        trace!("mail with id invalid {}", id);
        None
    })
}
