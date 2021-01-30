use std::collections::HashMap;

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

pub async fn process(mut receiver: Receiver<MailEvt>) -> crate::Result<()> {
    // This is the mail tank
    let mut mails: fnv::FnvHashMap<Ulid, Mail> = HashMap::default();

    let _mail_processing_task =
        spawn_task_and_swallow_log_errors("Mail broker".to_string(), async move {
            loop {
                if let Some(evt) = receiver.next().await {
                    log::trace!("processing MailEvt: {:?}", evt);
                    match evt {
                        // A new mail, add it to the list
                        MailEvt::NewMail(mail) => {
                            let _ = mails.insert(mail.get_id(), mail.clone());
                        }
                        // Want to retrieve the mail from this id
                        MailEvt::GetMail(sender, id) => {
                            sender.send(mails.get(&id).cloned()).await?;
                            drop(sender);
                        }
                        // Want to retrieve all mails
                        MailEvt::GetAll(sender) => {
                            for mail in mails.values().cloned() {
                                sender.send(mail).await?
                            }
                            drop(sender);
                        }
                        // Remove a mail by the id
                        MailEvt::Remove(sender, id) => {
                            sender.send(mails.remove(&id).map(|m| m.get_id())).await?;
                            drop(sender);
                        }
                        // Remove all mails
                        MailEvt::RemoveAll(sender) => {
                            let ids: Vec<Ulid> = mails.keys().cloned().collect();

                            for id in ids {
                                let _ = mails.remove(&id);
                                sender.send(id).await?;
                            }
                            drop(sender);
                        }
                    }
                }
            }
        });

    Ok(())
}

#[cfg(test)]
mod tests {
    use async_std::{channel, prelude::FutureExt, task};
    use std::time::Duration;

    use super::*;

    const DATA_SIMPLE: &str = r"Date: Sun, 22 Nov 2020 01:58:23 +0100
To: to@mail.com
From: from@mail.com
Subject: test Sun, 22 Nov 2020 01:58:23 +0100
Message-Id: <20201122015818.087219@example.net>
X-Mailer: swaks v20201014.0 jetmore.org/john/code/swaks/

This is a test mailing";

    fn new_mail() -> Mail {
        Mail::new("from@example.net", &["to@example.org".into()], DATA_SIMPLE)
    }

    #[test]
    fn test_mail_broker() {
        task::block_on(async {
            let (sender, receiver): crate::Channel<MailEvt> = channel::unbounded();

            // Launch and process commands
            assert!(process(receiver)
                .try_join(async move {
                    // -----------------------
                    // Test adding 1 mail
                    // -----------------------

                    let mail1: Mail = new_mail();
                    let mail2: Mail = new_mail();
                    let mail3: Mail = new_mail();

                    sender.send(MailEvt::NewMail(mail1.clone())).await.unwrap();
                    sender.send(MailEvt::NewMail(mail2.clone())).await.unwrap();
                    sender.send(MailEvt::NewMail(mail3.clone())).await.unwrap();

                    // -----------------------
                    // Test getting 1 mail by id
                    // -----------------------

                    let (s, mut r): crate::Channel<Option<Mail>> = channel::unbounded();

                    // Stream for unknown id
                    sender
                        .send(MailEvt::GetMail(s.clone(), Ulid::new()))
                        .await
                        .unwrap();

                    // Read unknown id result
                    let received_none: Option<Mail> = r.next().await.unwrap();
                    assert!(received_none.is_none());

                    // Stream for known id
                    sender
                        .send(MailEvt::GetMail(s, mail1.get_id()))
                        .await
                        .unwrap();

                    // Read known id result
                    let received_mail: Option<Mail> = r.next().await.unwrap();
                    assert!(received_mail.is_some());
                    assert_eq!(received_mail.unwrap().get_id(), mail1.get_id());

                    // -----------------------
                    // Retrieve all mails
                    // -----------------------

                    let (s, mut r): crate::Channel<Mail> = channel::unbounded();

                    sender.send(MailEvt::GetAll(s)).await.unwrap();
                    let mut nb_retrieved: usize = 0;
                    while let Some(received_mail) = r.next().await {
                        nb_retrieved += 1;
                        // result is not added ordered, so check with both added id
                        assert!(
                            received_mail.get_id() == mail1.get_id()
                                || received_mail.get_id() == mail2.get_id()
                                || received_mail.get_id() == mail3.get_id()
                        );
                    }
                    // 2 mails added, so must found 2 too...
                    assert_eq!(nb_retrieved, 3);

                    // -----------------------
                    // Test removing 1 mail
                    // -----------------------

                    let (s, mut r): crate::Channel<Option<Ulid>> = channel::unbounded();

                    //  ... for unknown id
                    sender
                        .send(MailEvt::Remove(s.clone(), Ulid::new()))
                        .await
                        .unwrap();
                    // Read unknown id result
                    let received_none: Option<Ulid> = r.next().await.unwrap();
                    assert!(received_none.is_none());
                    // 0 mails added, so must found 0 too...

                    // ... for known id
                    sender
                        .send(MailEvt::Remove(s, mail1.get_id()))
                        .await
                        .unwrap();
                    // Read known id result
                    let received_id: Option<Ulid> = r.next().await.unwrap();
                    assert_eq!(received_id.unwrap(), mail1.get_id());

                    // -----------------------
                    // Test removing all mails from tha pool
                    // -----------------------

                    let (s, mut r): crate::Channel<Ulid> = channel::unbounded();

                    // ... ask to remove all mails
                    sender.send(MailEvt::RemoveAll(s)).await.unwrap();
                    let mut nb_removed: usize = 0;
                    while let Some(received_mail) = r.next().await {
                        nb_removed += 1;
                        assert!(
                            received_mail == mail1.get_id()
                                || received_mail == mail2.get_id()
                                || received_mail == mail3.get_id()
                        );
                    }
                    // 2 mails added, so must found 2 too...
                    assert_eq!(nb_removed, 2);

                    Ok(())
                })
                .timeout(Duration::from_millis(500))
                .await
                .is_ok());
        });
    }
}
