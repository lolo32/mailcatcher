use std::borrow::Cow;

use tide::{sse::Sender, Body, Request};

use super::{MailEvt, MailShort, State};

#[derive(Debug)]
struct Event<'a> {
    name: &'a str,
    id: Option<&'a str>,
    data: Cow<'a, str>,
}

pub async fn handle_sse(req: Request<State>, sender: Sender) -> Result<(), tide::Error> {
    use futures::StreamExt;

    let mut sse_stream = req.state().chan_sse.clone();

    while let Some(mail_evt) = sse_stream.next().await {
        info!(
            "received new SSE notification, sending event to stream: {:?}",
            mail_evt
        );

        let data = match mail_evt {
            MailEvt::NewMail(mail) => {
                let mail = MailShort::new(&mail);
                Event {
                    name: "newMail",
                    id: None,
                    data: Cow::Owned(
                        Body::from_json(&mail)
                            .unwrap_or_else(|_| "".into())
                            .into_string()
                            .await?,
                    ),
                }
            }
            MailEvt::DelMail(id) => Event {
                name: "delMail",
                id: None,
                data: Cow::Owned(id.to_string()),
            },
            MailEvt::Ping => Event {
                name: "ping",
                id: None,
                data: Cow::Borrowed("💓"),
            },
        };
        trace!("data to send: {:?}", data);
        let sent = sender
            .send(data.name.as_ref(), data.data.as_ref(), data.id)
            .await;
        if sent.is_err() {
            warn!("Err, disconnected: {:?}", sent);
            break;
        } else {
            trace!("### Server-Sent Events sent");
        }
    }
    info!("### Exit /sse");
    Ok(())
}
