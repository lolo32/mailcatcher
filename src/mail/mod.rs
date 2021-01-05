use chrono::{DateTime, Utc};
use tide::prelude::json;
use ulid::Ulid;

use crate::encoding::decode_string;
use serde_json::Value;

pub mod broker;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub enum Type {
    // Raw mail content
    Raw,
    // Mail content that is in text format
    Text,
    // Mail content that is in HTML
    Html,
    Image(String, String),
    Other(String, String),
}

pub enum HeaderRepresentation {
    Raw,
    Humanized,
}

// Content of a mail
#[derive(Debug, Clone)]
pub struct Mail {
    // Id, internal used and JavaScript identifier
    id: Ulid,
    // From address
    from: String,
    // Array of receivers
    to: Vec<String>,
    // Subject of the mail
    subject: String,
    // Date of reception
    date: DateTime<Utc>,
    // Array of headers
    headers: Vec<String>,
    // Content of the mail, split in Type
    data: fnv::FnvHashMap<Type, String>,
}

impl Mail {
    // Create a new mail
    pub fn new(from: &str, to: &[String], data: &str) -> Self {
        let mut mail = Self {
            id: Ulid::new(),
            from: from.to_string(),
            to: to.to_vec(),
            subject: "(No subject)".to_string(),
            date: Utc::now(),
            headers: Vec::default(),
            data: fnv::FnvHashMap::default(),
        };

        // Store RAW mail content
        mail.data.insert(Type::Raw, data.to_string());

        let (headers, body) = Mail::split_header_body(data);

        // Parse the headers
        for header in headers.lines() {
            if &header[..1] == " " {
                // Multiline header, so append it into multiline content and last entry of the array
                if let Some(prev_line) = mail.headers.last_mut() {
                    prev_line.push_str("\r\n");
                    prev_line.push_str(header);
                    continue;
                }
            }
            // Single line, or first line of a multiline header
            mail.headers.push(header.to_string());
        }

        mail.data.insert(Type::Text, body);

        // Extract Date
        let date_header = mail.get_header_content("Date", HeaderRepresentation::Raw);
        if let Some(date_str) = date_header.first() {
            if let Ok(local_date) = DateTime::parse_from_rfc2822(date_str.as_str()) {
                mail.date = local_date.with_timezone(&Utc);
            }
        }

        // Extract Subject
        let subject_header = mail.get_header_content("Subject", HeaderRepresentation::Raw);
        if let Some(subject) = subject_header.first() {
            mail.subject = subject.clone();
        }

        mail
    }

    pub fn split_header_body(content: &str) -> (String, String) {
        let mut headers = String::new();

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
        let mut body = String::new();
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
    pub fn get_id(&self) -> Ulid {
        self.id
    }

    /// Retrieve the sender address
    pub fn from(&self) -> &String {
        &self.from
    }
    /// Retrieve the receivers addresses
    pub fn to(&self) -> &Vec<String> {
        &self.to
    }

    /// Retrieve mail date, either from the Date header or if not present from the reception time
    pub fn get_date(&self) -> DateTime<Utc> {
        self.date
    }

    /// Retrieve the subject
    pub fn get_subject(&self) -> &String {
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
    pub fn get_header_content(&self, key: &str, raw: HeaderRepresentation) -> Vec<String> {
        let key = format!("{}: ", key);
        let key_len = key.len();

        // Iterate over headers list to find the header
        self.get_headers(raw)
            .iter()
            // Filter over key name
            .filter(|header| header.len() > key_len && &header[..key_len] == key.as_str())
            // strip only to header content
            .map(|header| header[key_len..].to_string())
            .collect()
    }

    /// Retrieve headers list
    pub fn get_headers(&self, format: HeaderRepresentation) -> Vec<String> {
        self.headers
            .iter()
            .map(|header| match format {
                HeaderRepresentation::Raw => header.to_string(),
                HeaderRepresentation::Humanized => decode_string(header),
            })
            .collect()
    }

    /// Retrieve mail size
    pub fn get_size(&self) -> usize {
        self.data.get(&Type::Raw).unwrap().len()
    }

    /// Retrieve the data type part of the mail
    pub fn get_data(&self, type_: &Type) -> Option<&String> {
        self.data.get(type_)
    }

    pub fn summary(&self) -> Value {
        json!({
            "id": self.get_id().to_string(),
            "from": self.from().to_string(),
            "to": self.to().iter().map(|s| s.to_string()).collect::<Vec<String>>(),
            "subject": self.get_subject().to_string(),
            "date": self.get_date().timestamp(),
            "size": self.get_size(),
        })
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

    #[test]
    fn split_headers_body() {
        let mail = Mail::new("from@example.com", &["to@example.com".into()], DATA_SIMPLE);

        assert_eq!(mail.headers.len(), 6);
        assert_eq!(mail.from, "from@example.com");
        assert_eq!(mail.to.len(), 1);
        assert_eq!(mail.to.get(0).unwrap(), "to@example.com");
        assert!(mail.data.contains_key(&Type::Text));
        assert_eq!(
            mail.data.get(&Type::Text).unwrap(),
            "This is a test mailing\r\n\r\n"
        );
    }

    #[test]
    fn test_getting_datetime() {
        let mail = Mail::new("", &[], DATA_SIMPLE);

        let date = mail.get_date();
        assert_eq!(date.timezone(), Utc);

        let dt = Utc.ymd(2020, 11, 22).and_hms(0, 58, 23);
        assert_eq!(date, dt);
    }

    #[test]
    fn get_text() {
        let mail = Mail::new("", &[], DATA_SIMPLE);

        let text = mail.get_text();

        assert!(text.is_some());
        assert_eq!(text.unwrap(), &"This is a test mailing\r\n\r\n".to_string());

        assert!(mail.get_html().is_none());
    }
}
