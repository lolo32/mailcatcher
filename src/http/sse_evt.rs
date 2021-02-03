use std::borrow::Cow;

use async_std::task;
use tide::Body;
use ulid::Ulid;

use crate::mail::Mail;

/// Events that can be sent to SSE
#[derive(Clone, Debug)]
pub enum SseEvt {
    /// A new mail has arrived
    NewMail(Mail),
    /// A mail was deleted
    DelMail(Ulid),
    /// Ping to test connection
    Ping,
}

/// Data that can be sent to client browsers with SSE
#[derive(Debug)]
pub struct SseData<'a> {
    /// SSE event name
    pub name: &'a str,
    /// SSE event data content
    pub data: Cow<'a, str>,
}

/// Convert from `SseEvt` to `SseData`
impl From<SseEvt> for SseData<'_> {
    fn from(sse_evt: SseEvt) -> Self {
        match sse_evt {
            SseEvt::NewMail(mail) => {
                let mail: serde_json::Value = mail.summary();
                let data: String = task::block_on(async move {
                    Body::from_json(&mail)
                        .unwrap_or_else(|_| "".into())
                        .into_string()
                        .await
                        .expect("json new mail")
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
        let sse_evt: SseEvt = SseEvt::Ping;
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "ping");
        assert_eq!(data.data, "\u{1f493}");

        let id: Ulid = Ulid::new();
        let sse_evt: SseEvt = SseEvt::DelMail(id);
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "delMail");
        assert_eq!(data.data, id.to_string());

        let mail: Mail = Mail::new(
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
        let id: Ulid = mail.get_id();
        let sse_evt: SseEvt = SseEvt::NewMail(mail);
        let data: SseData = sse_evt.into();
        assert_eq!(data.name, "newMail");
        assert_eq!(data.data, format!("{{\"date\":1606006703,\"from\":\"from@example.org\",\"id\":\"{}\",\"size\":248,\"subject\":\"test Sun, 22 Nov 2020 01:58:23 +0100\",\"to\":[\"to@example.net\"]}}", id));
    }
}
