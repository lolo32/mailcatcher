use std::ops::Sub;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use fake::{
    faker::{
        chrono::en::DateTimeBetween,
        internet::en::{FreeEmailProvider, SafeEmail},
        lorem::en::{Paragraphs, Words},
        name::en::Name,
    },
    Fake,
};
use serde_json::Value;
use textwrap::wrap;
use tide::prelude::json;
use ulid::Ulid;

use crate::encoding::decode_string;

/// Mail storage broker
pub mod broker;

/// Describe the data type that is held
#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub enum Type {
    /// Raw mail content
    Raw,
    /// Mail content that is in text format
    Text,
    /// Mail content that is in HTML
    Html,
    //Image(String, String),
    //Other(String, String),
}

/// How to export mail header
/// * Raw: like it was received
/// * Humanized: decode it
pub enum HeaderRepresentation {
    /// Header like it was received
    Raw,
    /// Header decoded
    Humanized,
}

/// Content of a mail
#[derive(Debug, Clone)]
pub struct Mail {
    /// Id, internal used and JavaScript identifier
    id: Ulid,
    /// From address
    from: String,
    /// Array of receivers
    to: Vec<String>,
    /// Subject of the mail
    subject: String,
    /// Date of reception
    date: DateTime<Utc>,
    /// Array of headers
    headers: Vec<String>,
    /// Content of the mail, split in Type
    data: fnv::FnvHashMap<Type, String>,
}

impl Mail {
    /// Create a new mail
    pub fn new(from: &str, to: &[String], data: &str) -> Self {
        let mut mail = Self {
            id: Ulid::new(),
            from: from.to_owned(),
            to: to.to_vec(),
            subject: "(No subject)".to_owned(),
            date: Utc::now(),
            headers: Vec::default(),
            data: fnv::FnvHashMap::default(),
        };

        // Store RAW mail content
        let _ = mail.data.insert(Type::Raw, data.to_owned());

        let (headers, body): (String, String) = Self::split_header_body(data);

        // Parse the headers
        for header in headers.lines() {
            #[allow(clippy::indexing_slicing)]
            if &header[..1] == " " {
                // Multiline header, so append it into multiline content and last entry of the array
                if let Some(prev_line) = mail.headers.last_mut() {
                    prev_line.push_str("\r\n");
                    prev_line.push_str(header);
                    continue;
                }
            }
            // Single line, or first line of a multiline header
            mail.headers.push(header.to_owned());
        }

        let _ = mail.data.insert(Type::Text, body);

        // Extract Date
        let date_header: Vec<String> = mail.get_header_content("Date", &HeaderRepresentation::Raw);
        if let Some(date_str) = date_header.first() {
            if let Ok(local_date) = DateTime::parse_from_rfc2822(date_str.as_str()) {
                mail.date = local_date.with_timezone(&Utc);
            }
        }

        // Extract Subject
        let subject_header: Vec<String> =
            mail.get_header_content("Subject", &HeaderRepresentation::Raw);
        if let Some(subject) = subject_header.first() {
            mail.subject = subject.clone();
        }

        mail
    }

    /// Split the string, returning a tuple that is the headers then the body
    pub fn split_header_body(content: &str) -> (String, String) {
        let mut headers: String = String::new();

        // Iterator over lines
        let mut lines = content.lines();

        // Parse the headers
        while let Some(line) = lines.next() {
            if line.is_empty() {
                // Empty line = end of headers, so exit this loop
                break;
            }
            headers.push_str(line);
            headers.push_str("\r\n");
        }

        // Parse the body of the mail
        let mut body: String = String::new();
        if let Some(line) = lines.next() {
            body.push_str(line);
        }
        for line in lines {
            body.push_str("\r\n");
            body.push_str(line);
        }

        (headers.trim_end().into(), body)
    }

    /// Retrieve the ID of the mail
    pub const fn get_id(&self) -> Ulid {
        self.id
    }

    /// Retrieve the sender address
    pub const fn from(&self) -> &String {
        &self.from
    }
    /// Retrieve the receivers addresses
    pub const fn to(&self) -> &Vec<String> {
        &self.to
    }

    /// Retrieve mail date, either from the Date header or if not present from the reception time
    pub const fn get_date(&self) -> DateTime<Utc> {
        self.date
    }

    /// Retrieve the subject
    pub const fn get_subject(&self) -> &String {
        &self.subject
    }

    /// Retrieve the content in text format
    pub fn get_text(&self) -> Option<&String> {
        self.data.get(&Type::Text)
    }

    /// Retrieve the content in html format
    pub fn get_html(&self) -> Option<&String> {
        self.data.get(&Type::Html)
    }

    /// Retrieve the header content, from the key name
    /// The data can be in literal format or humanized
    #[allow(clippy::indexing_slicing)]
    pub fn get_header_content(&self, key: &str, raw: &HeaderRepresentation) -> Vec<String> {
        let key: String = format!("{}: ", key);
        let key_len: usize = key.len();

        // Iterate over headers list to find the header
        self.get_headers(raw)
            .iter()
            // Filter over key name
            .filter_map(|header| {
                if header.find(' ') == Some(key_len.sub(1)) && &header[..key_len] == key.as_str() {
                    // strip only to header content
                    Some(header[key_len..].to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Retrieve headers list
    pub fn get_headers(&self, format: &HeaderRepresentation) -> Vec<String> {
        self.headers
            .iter()
            .map(|header| match *format {
                HeaderRepresentation::Raw => header.to_string(),
                HeaderRepresentation::Humanized => decode_string(header),
            })
            .collect()
    }

    /// Retrieve mail size
    pub fn get_size(&self) -> usize {
        #[allow(clippy::indexing_slicing)]
        self.data[&Type::Raw].as_bytes().len()
    }

    /// Retrieve the data type part of the mail
    pub fn get_data(&self, type_: &Type) -> Option<&String> {
        self.data.get(type_)
    }

    /// Return a symplification of the email, for sending it over JSON
    pub fn summary(&self) -> Value {
        json!({
            "id": self.get_id().to_string(),
            "from": self.from().to_string(),
            "to": self.to(),
            "subject": self.get_subject().to_string(),
            "date": self.get_date().timestamp(),
            "size": self.get_size(),
        })
    }

    /// Generate a fake email, based on Lorem Ispum random content
    #[allow(unused, clippy::indexing_slicing)]
    pub fn fake() -> Self {
        /// Put the first character to uppercase, then do not touch the following
        fn make_first_uppercase(s: &str) -> String {
            format!("{}{}", s[0..1].to_uppercase(), s[1..].to_owned())
        }

        // Expeditor mail address
        let from: String = SafeEmail().fake();
        let from_name: String = Name().fake();
        // Recipient mail address
        let to: String = SafeEmail().fake();
        let to_name: String = Name().fake();
        // Mail subject
        let subject: Vec<String> = Words(5..10).fake();
        let subject: String = make_first_uppercase(&subject.join(" "));
        // Body content
        let body: String = {
            let mut body: Vec<String> = Paragraphs(1..8).fake();
            body[0] = format!("Lorem ipsum dolor sit amet, {}", body[0]);
            let body: Vec<String> = body
                .iter()
                .map(|s| {
                    s.split('\n')
                        .map(|s| make_first_uppercase(s))
                        .collect::<Vec<String>>()
                        .join("  ")
                })
                .collect();
            wrap(&body.join("\r\n\r\n"), 72).join("\r\n")
        };
        // Mail Date
        let date: DateTime<Utc> = DateTimeBetween(
            Utc.from_utc_datetime(&NaiveDateTime::from_timestamp(
                Utc::now().timestamp().sub(15_552_000),
                0,
            )),
            Utc::now(),
        )
        .fake();

        let mail_full: String = format!(
            "Date: {}\r\nFrom: {}<{}>\r\nTo: {}<{}>\r\nSubject: {}\r\nX-Mailer: mailcatcher/Fake\r\nMessage-Id: <{}.{}@{}>\r\n\r\n{}",
            date.to_rfc2822(),
            from_name,
            from,
            to_name,
            to,
            subject,
            date.timestamp(),
            date.timestamp_millis(),
            FreeEmailProvider().fake::<String>(),
            body,
        );
        log::trace!("Faking new mail:\n{}", mail_full);

        Self::new(
            &format!("{}<{}>", from_name, from),
            &[format!("{}<{}>", to_name, to)],
            &mail_full,
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const DATA_SIMPLE: &str = r"Date: Sun, 22 Nov 2020 01:58:23 +0100
To: to@mail.com
From: from@mail.com
Subject: test Sun, 22 Nov 2020 01:58:23 +0100
Message-Id: <20201122015818.087219@example.net>
X-Mailer: swaks v20201014.0 jetmore.org/john/code/swaks/

This is a test mailing


";
    const DATA_COMPLEX: &str = r"From: =?US-ASCII?Q?Keith_Moore?= <moore@cs.utk.edu>;
To: =?ISO-8859-1?Q?Keld_J=F8rn_Simonsen?= <keld@dkuug.dk>;
CC: =?ISO-8859-1?Q?Andr=E9?= Pirard <PIRARD@vm1.ulg.ac.be>;
Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?=
 =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=

This is the content of this mail... but it says nothing now.";

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn split_headers_body() {
        crate::test::log_init();

        let mail: Mail = Mail::new("from@example.com", &["to@example.com".into()], DATA_SIMPLE);

        assert_eq!(mail.headers.len(), 6);
        assert_eq!(mail.from, "from@example.com");
        assert_eq!(mail.to.len(), 1);
        assert_eq!(mail.to[0], "to@example.com");
        assert!(mail.data.contains_key(&Type::Text));
        assert_eq!(mail.data[&Type::Text], "This is a test mailing\r\n\r\n");
    }

    #[test]
    fn test_getting_datetime() {
        crate::test::log_init();

        let mail: Mail = Mail::new("", &[], DATA_SIMPLE);

        let date: DateTime<Utc> = mail.get_date();
        assert_eq!(date.timezone(), Utc);

        let dt: DateTime<Utc> = Utc.ymd(2020, 11, 22).and_hms(0, 58, 23);
        assert_eq!(date, dt);
    }

    #[test]
    fn get_text() {
        crate::test::log_init();

        let mail: Mail = Mail::new("", &[], DATA_SIMPLE);

        let text: Option<&String> = mail.get_text();

        assert_eq!(
            text.expect("mail text body"),
            &"This is a test mailing\r\n\r\n".to_owned()
        );

        assert!(mail.get_html().is_none());
    }

    #[test]
    fn summary_is_json() {
        crate::test::log_init();

        let mail: Mail = Mail::new("from@example.org", &["to@example.net".into()], DATA_SIMPLE);
        let summary: String = mail.summary().to_string();

        assert_eq!(
            summary,
            format!(
                r#"{{"date":1606006703,"from":"from@example.org","id":"{}","size":251,"subject":"test Sun, 22 Nov 2020 01:58:23 +0100","to":["to@example.net"]}}"#,
                mail.id
            )
        );
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn multiline_header_content_and_humanized() {
        crate::test::log_init();

        let mail: Mail = Mail::new("from@example.org", &["to@example.net".into()], DATA_COMPLEX);
        let subject_raw: Vec<String> =
            mail.get_header_content("Subject", &HeaderRepresentation::Raw);

        assert_eq!(subject_raw.len(), 1);
        let subject_raw: &String = &subject_raw[0];
        assert_eq!(subject_raw, "=?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?=\r\n =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=");

        let subject_human: Vec<String> =
            mail.get_header_content("Subject", &HeaderRepresentation::Humanized);

        assert_eq!(subject_human.len(), 1);
        let subject_human: &String = &subject_human[0];
        assert_eq!(
            subject_human,
            "If you can read this you understand the example."
        );
    }

    #[test]
    fn get_data() {
        crate::test::log_init();

        let mail: Mail = Mail::new("from@example.org", &["to@example.net".into()], DATA_COMPLEX);

        assert!(mail.get_data(&Type::Html).is_none());

        let content: Option<&String> = mail.get_data(&Type::Text);
        assert_eq!(
            content.expect("mail body"),
            "This is the content of this mail... but it says nothing now."
        );
    }
}
