use tide::{sse::Sender, Body, Request};

use super::{MailEvt, MailShort, State};

pub async fn sse(req: Request<State>, sender: Sender) -> Result<(), tide::Error> {
    use futures::StreamExt;

    let mut chan_sse = req.state().chan_sse.clone();
    trace!("### chan_sse: {:?}", chan_sse);

    while let Some(mail_evt) = chan_sse.next().await {
        info!(
            "SSE: received new Mail notification, sending to stream: {:?}",
            mail_evt
        );

        let data = match mail_evt {
            MailEvt::NewMail(mail) => {
                let mail = MailShort::new(&mail);
                (
                    "newMail",
                    Body::from_json(&mail)
                        .unwrap_or_else(|_| "".into())
                        .into_string()
                        .await?,
                )
            }
            MailEvt::DelMail(id) => ("delMail", id.to_string()),
            MailEvt::Ping => ("ping", "💓".to_string()),
        };
        trace!("SSE: data to send: {:?}", data);
        let sent = sender.send(data.0, data.1, None).await;
        if sent.is_err() {
            warn!("SSE: Err, disconnected: {:?}", sent);
            break;
        } else {
            trace!("### Server-Sent Events sent");
        }
    }
    info!("### Exit /sse");
    Ok(())
}
