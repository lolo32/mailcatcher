use std::borrow::Cow;

use async_std::task;
use tide::Body;
use ulid::Ulid;

use crate::{http::MailShort, mail::Mail};

/// Events that can be sent to SSE
#[derive(Clone, Debug)]
pub enum SseEvt {
    NewMail(Mail),
    DelMail(Ulid),
    Ping,
}

/// Data that can be sent to client browsers with SSE
#[derive(Debug)]
pub struct SseData<'a> {
    pub name: &'a str,
    pub id: Option<&'a str>,
    pub data: Cow<'a, str>,
}

/// Convert from SseEvt to SseData
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
