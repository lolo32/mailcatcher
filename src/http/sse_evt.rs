use ulid::Ulid;

use crate::http::MailShort;
use crate::mail::Mail;
use async_std::task;
use std::borrow::Cow;
use tide::Body;

#[derive(Clone, Debug)]
pub enum SseEvt {
    NewMail(Mail),
    DelMail(Ulid),
    Ping,
}

#[derive(Debug)]
pub struct SseData<'a> {
    pub name: &'a str,
    pub id: Option<&'a str>,
    pub data: Cow<'a, str>,
}

impl<'a> From<SseEvt> for SseData<'a> {
    fn from(sse_evt: SseEvt) -> Self {
        match sse_evt {
            SseEvt::NewMail(mail) => {
                let mail = MailShort::new(&mail);
                let data = task::block_on(async move {
                    Body::from_json(&mail)
                        .unwrap_or_else(|_| "".into())
                        .into_string()
                        .await
                        .unwrap()
                });
                SseData {
                    name: "newMail",
                    id: None,
                    data: Cow::Owned(data),
                }
            }
            SseEvt::DelMail(id) => SseData {
                name: "delMail",
                id: None,
                data: Cow::Owned(id.to_string()),
            },
            SseEvt::Ping => SseData {
                name: "ping",
                id: None,
                data: Cow::Borrowed("💓"),
            },
        }
    }
}
