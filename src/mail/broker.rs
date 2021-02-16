use std::collections::HashMap;

use async_std::channel::{Receiver, Sender};
use futures::StreamExt;
use ulid::Ulid;

use crate::mail::Mail;

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

    /// Mail storage broker. All communication is from the `Receiver` stream
    pub async fn process(mut self) -> crate::Result<()> {
        loop {
            if let Some(evt) = self.receiver.next().await {
                log::trace!("processing MailEvt: {:?}", evt);
                match evt {
                    // A new mail, add it to the list
                    MailEvt::NewMail(mail) => {
                        log::trace!("Adding new mail");
                        let _ = self.mails.insert(mail.get_id(), mail.clone());
                    }
                    // Want to retrieve the mail from this id
                    MailEvt::GetMail(sender, id) => {
                        let mail = self.mails.get(&id);
                        log::trace!("Mail found: {:?}", mail);
                        sender.send(mail.cloned()).await?;
                        drop(sender);
                    }
                    // Want to retrieve all mails
                    MailEvt::GetAll(sender) => {
                        log::trace!("All mails retrieved");
                        for mail in self.mails.values().cloned() {
                            sender.send(mail).await?
                        }
                        drop(sender);
                    }
                    // Remove a mail by the id
                    MailEvt::Remove(sender, id) => {
                        let mail_id = self.mails.remove(&id).map(|m| m.get_id());
                        log::trace!("Mail deleted: {:?}", mail_id);
                        sender.send(mail_id).await?;
                        drop(sender);
                    }
                    // Remove all mails
                    MailEvt::RemoveAll(sender) => {
                        let ids: Vec<Ulid> = self.mails.keys().cloned().collect();
                        log::trace!("All mails removed");

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
    use async_std::{channel, prelude::FutureExt, task};

    use super::*;

    struct Init {
        mails: Vec<Mail>,
        sender: Sender<MailEvt>,
        broker: MailTank,
    }

    async fn init() -> crate::Result<Init> {
        // Enable crate log output
        crate::test::log_init();

        let (sender, receiver): crate::Channel<MailEvt> = channel::unbounded();

        // Generate 3 mails
        let mut mails = Vec::new();
        for _ in 0..3 {
            let mail = Mail::fake();
            mails.push(mail.clone());
            sender.send(MailEvt::NewMail(mail)).await?;
        }

        let broker = MailTank::new(receiver);

        Ok(Init {
            mails,
            sender,
            broker,
        })
    }

    #[test]
    fn get_one_mail() -> std::io::Result<()> {
        #[allow(clippy::indexing_slicing, clippy::panic)]
        async fn the_test(mails: Vec<Mail>, sender: Sender<MailEvt>) -> crate::Result<()> {
            // -----------------------
            // Test getting 1 mail by id
            // -----------------------

            // Stream for unknown id
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Mail>> = channel::unbounded();

                sender.send(MailEvt::GetMail(s, Ulid::new())).await?;

                // Read unknown id result
                let received_none: Option<Mail> = r.next().await.ok_or("no response received")?;
                assert!(received_none.is_none());
            }

            // Stream for known id
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Mail>> = channel::unbounded();

                sender.send(MailEvt::GetMail(s, mails[0].get_id())).await?;

                // Read known id result
                let received_mail: Option<Mail> = r.next().await.ok_or("no response received")?;
                assert!(received_mail.is_some());
                assert_eq!(
                    received_mail.expect("mail exists").get_id(),
                    mails[0].get_id()
                );
            }

            Ok(())
        }

        let Init {
            mails,
            sender,
            broker,
        } = task::block_on(init()).expect("Init");

        crate::test::with_timeout(5_000, broker.process().race(the_test(mails, sender)))
    }

    #[test]
    fn get_all_mails() -> std::io::Result<()> {
        #[allow(clippy::indexing_slicing, clippy::panic)]
        async fn the_test(mails: Vec<Mail>, sender: Sender<MailEvt>) -> crate::Result<()> {
            // -----------------------
            // Test getting all mails
            // -----------------------

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

            Ok(())
        }

        let Init {
            mails,
            sender,
            broker,
        } = task::block_on(init()).expect("Init");

        crate::test::with_timeout(5_000, broker.process().race(the_test(mails, sender)))
    }

    #[test]
    fn remove_one_mail() -> std::io::Result<()> {
        #[allow(clippy::indexing_slicing, clippy::panic)]
        async fn the_test(mut mails: Vec<Mail>, sender: Sender<MailEvt>) -> crate::Result<()> {
            // -----------------------
            // Test removing 1 mail
            // -----------------------

            //  ... for unknown id
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Ulid>> = channel::unbounded();

                sender.send(MailEvt::Remove(s, Ulid::new())).await?;
                // Read unknown id result
                let received_none: Option<Ulid> = r.next().await.ok_or("no received response")?;
                assert!(received_none.is_none());
                // 0 mails added, so must found 0 too...
            }

            // ... for known id
            {
                // Stream channel to communicate
                let (s, mut r): crate::Channel<Option<Ulid>> = channel::unbounded();

                let removed_mail = mails.remove(0);

                sender
                    .send(MailEvt::Remove(s, removed_mail.get_id()))
                    .await?;
                // Read known id result
                let received_id: Option<Ulid> = r.next().await.ok_or("no received response")?;
                assert_eq!(received_id.expect("id"), removed_mail.get_id());
            }

            Ok(())
        }

        let Init {
            mails,
            sender,
            broker,
        } = task::block_on(init()).expect("Init");

        crate::test::with_timeout(5_000, broker.process().race(the_test(mails, sender)))
    }

    #[test]
    fn remove_all_mails() -> std::io::Result<()> {
        #[allow(clippy::indexing_slicing, clippy::panic)]
        async fn the_test(mails: Vec<Mail>, sender: Sender<MailEvt>) -> crate::Result<()> {
            // -----------------------
            // Test removing all mails from tha pool
            // -----------------------

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

            Ok(())
        }

        let Init {
            mails,
            sender,
            broker,
        } = task::block_on(init()).expect("Init");

        crate::test::with_timeout(5_000, broker.process().race(the_test(mails, sender)))
    }
}
