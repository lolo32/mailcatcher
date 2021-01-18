use std::borrow::Cow;

use async_std::task;
use tide::Body;
use ulid::Ulid;

use crate::mail::Mail;

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
    pub data: Cow<'a, str>,
}

/// Convert from SseEvt to SseData
impl<'a> From<SseEvt> for SseData<'a> {
    fn from(sse_evt: SseEvt) -> Self {
        match sse_evt {
            SseEvt::NewMail(mail) => {
                let mail = mail.summary();
                let data = task::block_on(async move {
                    Body::from_json(&mail)
                        .unwrap_or_else(|_| "".into())
                        .into_string()
                        .await
                        .unwrap()
                });
                SseData {
                    name: "newMail",
                    data: Cow::Owned(data),
                }
            }
            SseEvt::DelMail(id) => SseData {
                name: "delMail",
                data: Cow::Owned(id.to_string()),
            },
            SseEvt::Ping => SseData {
                name: "ping",
                data: Cow::Borrowed("\u{1f493}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_to_sse_data() {
        let sse_evt = SseEvt::Ping;
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "ping");
        assert_eq!(data.data, "💓");

        let id = Ulid::new();
        let sse_evt = SseEvt::DelMail(id);
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "delMail");
        assert_eq!(data.data, id.to_string());

        let mail = Mail::new(
            "from@example.org",
            &["to@example.net".into()],
            r"Date: Sun, 22 Nov 2020 01:58:23 +0100
To: to@mail.com
From: from@mail.com
Subject: test Sun, 22 Nov 2020 01:58:23 +0100
Message-Id: <20201122015818.087219@example.net>
X-Mailer: swaks v20201014.0 jetmore.org/john/code/swaks/

This is a test mailing",
        );
        let id = mail.get_id();
        let sse_evt = SseEvt::NewMail(mail);
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "newMail");
        assert_eq!(data.data, format!("{{\"date\":1606006703,\"from\":\"from@example.org\",\"id\":\"{}\",\"size\":248,\"subject\":\"test Sun, 22 Nov 2020 01:58:23 +0100\",\"to\":[\"to@example.net\"]}}", id));
    }
}
