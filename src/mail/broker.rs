use std::collections::HashMap;

use async_std::channel::{Receiver, Sender};
use futures::StreamExt;
use ulid::Ulid;

use crate::{mail::Mail, utils::spawn_task_and_swallow_log_errors};

/// Mail events sent from the SMTP (for `NewMail`) or HTTP side for the other from streams
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

/// Mail tank broker
pub struct MailTank {
    /// Mails tank
    mails: fnv::FnvHashMap<Ulid, Mail>,
    /// Channel to access the tank from the outside
    receiver: Receiver<MailEvt>,
}

impl MailTank {
    /// Instantiate a new broker
    pub fn new(receiver: Receiver<MailEvt>) -> Self {
        Self {
            mails: HashMap::default(),
            receiver,
        }
    }

    /// Spawn an async task managing the mails tank
    pub async fn task(self, task_name: &str) -> crate::Result<()> {
        let _mail_tank_task =
            spawn_task_and_swallow_log_errors(task_name.to_owned(), self.process())?;

        Ok(())
    }

    /// Mail storage broker. All communication is from the `Receiver` stream
    pub async fn process(mut self) -> crate::Result<()> {
        loop {
            if let Some(evt) = self.receiver.next().await {
                log::trace!("processing MailEvt: {:?}", evt);
                match evt {
                    // A new mail, add it to the list
                    MailEvt::NewMail(mail) => {
                        let _ = self.mails.insert(mail.get_id(), mail.clone());
                    }
                    // Want to retrieve the mail from this id
                    MailEvt::GetMail(sender, id) => {
                        sender.send(self.mails.get(&id).cloned()).await?;
                        drop(sender);
                    }
                    // Want to retrieve all mails
                    MailEvt::GetAll(sender) => {
                        for mail in self.mails.values().cloned() {
                            sender.send(mail).await?
                        }
                        drop(sender);
                    }
                    // Remove a mail by the id
                    MailEvt::Remove(sender, id) => {
                        sender
                            .send(self.mails.remove(&id).map(|m| m.get_id()))
                            .await?;
                        drop(sender);
                    }
                    // Remove all mails
                    MailEvt::RemoveAll(sender) => {
                        let ids: Vec<Ulid> = self.mails.keys().cloned().collect();

                        for id in ids {
                            let _ = self.mails.remove(&id);
                            sender.send(id).await?;
                        }
                        drop(sender);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_std::{channel, prelude::FutureExt};

    use super::*;

    #[test]
    fn mail_broker() -> std::io::Result<()> {
        #[allow(clippy::indexing_slicing, clippy::panic)]
        async fn the_test(sender: Sender<MailEvt>) -> crate::Result<()> {
            // Mails pull to compare
            let mut mails = Vec::new();

            // -----------------------
            // Test adding mails
            // -----------------------
            for _ in 0..3 {
                let mail = Mail::fake();
                mails.push(mail.clone());
                sender.send(MailEvt::NewMail(mail)).await?;
            }

            // -----------------------
            // Test getting 1 mail by id
            // -----------------------
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Mail>> = channel::unbounded();

                // Stream for unknown id
                sender
                    .send(MailEvt::GetMail(s.clone(), Ulid::new()))
                    .await?;

                // Read unknown id result
                let received_none: Option<Mail> = r.next().await.ok_or("no response received")?;
                assert!(received_none.is_none());

                // Stream for known id
                sender.send(MailEvt::GetMail(s, mails[0].get_id())).await?;

                // Read known id result
                let received_mail: Option<Mail> = r.next().await.ok_or("no response received")?;
                assert_eq!(
                    received_mail.ok_or("mail does not exists")?.get_id(),
                    mails[0].get_id()
                );
            }

            // -----------------------
            // Retrieve all mails
            // -----------------------
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Mail> = channel::unbounded();

                sender.send(MailEvt::GetAll(s)).await?;
                let mut mail_retrieved = Vec::new();
                while let Some(received_mail) = r.next().await {
                    // result is not added ordered
                    mail_retrieved.push(received_mail.get_id());
                }
                // 3 mails added, so must found 3 too...
                assert_eq!(mail_retrieved.len(), mails.len());
                for mail in &mails {
                    assert!(
                        mail_retrieved.contains(&mail.get_id()),
                        "Received mail id does not exists"
                    );
                }
            }

            // -----------------------
            // Test removing 1 mail
            // -----------------------
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Ulid>> = channel::unbounded();

                //  ... for unknown id
                sender.send(MailEvt::Remove(s.clone(), Ulid::new())).await?;
                // Read unknown id result
                let received_none: Option<Ulid> = r.next().await.ok_or("no received response")?;
                assert!(received_none.is_none());
                // 0 mails added, so must found 0 too...

                let removed_mail = mails.remove(0);

                // ... for known id
                sender
                    .send(MailEvt::Remove(s, removed_mail.get_id()))
                    .await?;
                // Read known id result
                let received_id: Option<Ulid> = r.next().await.ok_or("no received response")?;
                assert_eq!(
                    received_id.ok_or("id does not exists")?,
                    removed_mail.get_id()
                );
            }

            // -----------------------
            // Test removing all mails from tha pool
            // -----------------------
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Ulid> = channel::unbounded();

                // ... ask to remove all mails
                sender.send(MailEvt::RemoveAll(s)).await?;
                let mut mail_removed = Vec::new();
                while let Some(received_mail) = r.next().await {
                    mail_removed.push(received_mail);
                }
                // 2 mails added, so must found 2 too...
                assert_eq!(mail_removed.len(), mails.len());

                for mail in mails {
                    assert!(
                        mail_removed.contains(&mail.get_id()),
                        "Removed mail id does not exists"
                    );
                }
            }

            Ok(())
        }

        crate::test::log_init();

        let (sender, receiver): crate::Channel<MailEvt> = channel::unbounded();

        crate::test::with_timeout(
            5_000,
            MailTank::new(receiver).process().race(the_test(sender)),
        )
    }
}
