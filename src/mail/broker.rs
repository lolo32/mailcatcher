use crate::http::sse_evt::SseEvt;
use crate::mail::Mail;
use crate::utils::spawn_task_and_swallow_log_errors;
use async_std::channel::{Receiver, Sender};
use broadcaster::BroadcastChannel;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use std::collections::HashMap;
use ulid::Ulid;

#[derive(Clone, Debug)]
pub enum MailEvt {
    NewMail(Mail),
    GetMail(Ulid, Sender<Option<Mail>>),
    GetAll(Sender<Mail>),
    Remove(Ulid, Sender<Option<Ulid>>),
    RemoveAll,
}

pub async fn mail_broker(
    sse_stream: BroadcastChannel<SseEvt, UnboundedSender<SseEvt>, UnboundedReceiver<SseEvt>>,
    mut receiver: Receiver<MailEvt>,
) {
    let mut mails: HashMap<Ulid, Mail> = Default::default();

    spawn_task_and_swallow_log_errors("Mail broker".to_string(), async move {
        while let Some(evt) = receiver.next().await {
            trace!("processing MailEvt: {:?}", evt);
            match evt {
                MailEvt::NewMail(mail) => {
                    mails.insert(mail.id, mail.clone());
                    // Notify javascript side by SSE
                    match sse_stream.send(&SseEvt::NewMail(mail)).await {
                        Ok(_) => trace!(">>> New mail notification sent to channel"),
                        Err(e) => error!(">>> Err new mail notification to channel: {:?}", e),
                    }
                }
                MailEvt::GetMail(id, sender) => {
                    sender.send(mails.get(&id).cloned()).await?;
                }
                MailEvt::GetAll(sender) => {
                    for mail in mails.values().cloned() {
                        sender.send(mail).await?
                    }
                }
                MailEvt::Remove(id, sender) => {
                    sender
                        .send(if mails.remove(&id).is_some() {
                            Some(id)
                        } else {
                            None
                        })
                        .await?;
                }
                MailEvt::RemoveAll => {
                    let ids = mails.keys().cloned().collect::<Vec<_>>();

                    for id in ids {
                        mails.remove(&id);
                        sse_stream.send(&SseEvt::DelMail(id)).await?;
                    }
                }
            }
        }
        Ok(())
    });
}
