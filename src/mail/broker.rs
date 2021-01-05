use async_std::channel::{Receiver, Sender};
use futures::StreamExt;
use ulid::Ulid;

use crate::{mail::Mail, utils::spawn_task_and_swallow_log_errors};

#[derive(Clone, Debug)]
pub enum MailEvt {
    /// Add a new mail to the tank
    NewMail(Mail),
    /// Get a mail from the id
    GetMail(Sender<Option<Mail>>, Ulid),
    /// Get all mails in the tank
    GetAll(Sender<Mail>),
    /// Remove a mail by it's id
    Remove(Sender<Option<Ulid>>, Ulid),
    /// Clear the mail tank
    RemoveAll(Sender<Ulid>),
}

pub async fn mail_broker(mut receiver: Receiver<MailEvt>) -> crate::Result<()> {
    // This is the mail tank
    let mut mails: fnv::FnvHashMap<Ulid, Mail> = Default::default();

    spawn_task_and_swallow_log_errors("Mail broker".to_string(), async move {
        loop {
            if let Some(evt) = receiver.next().await {
                trace!("processing MailEvt: {:?}", evt);
                match evt {
                    // A new mail, add it to the list
                    MailEvt::NewMail(mail) => {
                        mails.insert(mail.id, mail.clone());
                    }
                    // Want to retrieve the mail from this id
                    MailEvt::GetMail(sender, id) => {
                        sender.send(mails.get(&id).cloned()).await?;
                    }
                    // Want to retrieve all mails
                    MailEvt::GetAll(sender) => {
                        for mail in mails.values().cloned() {
                            sender.send(mail).await?
                        }
                    }
                    // Remove a mail by the id
                    MailEvt::Remove(sender, id) => {
                        sender.send(mails.remove(&id).map(|m| m.id)).await?;
                    }
                    // Remove all mails
                    MailEvt::RemoveAll(sender) => {
                        let ids = mails.keys().cloned().collect::<Vec<_>>();

                        for id in ids {
                            mails.remove(&id);
                            sender.send(id).await?;
                        }
                    }
                }
            }
        }
    });

    Ok(())
}
