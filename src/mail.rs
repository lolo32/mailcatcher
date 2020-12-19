use chrono::{DateTime, Utc};
use ulid::Ulid;

use crate::encoding::decode_string;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub enum Type {
    Raw,
    Text,
    Html,
    Image(String, String),
    Other(String, String),
}

#[derive(Debug, Clone)]
pub struct Mail {
    id: Ulid,
    from: String,
    to: Vec<String>,
    subject: String,
    date: DateTime<Utc>,
    headers: Vec<String>,
    data: fnv::FnvHashMap<Type, String>,
}

impl Mail {
    pub fn new(from: &str, to: &[String], data: &str) -> Self {
        let mut this = Self {
            id: Ulid::new(),
            from: from.to_string(),
            to: to.to_vec(),
            subject: "(No subject)".to_string(),
            date: Utc::now(),
            headers: Vec::default(),
            data: fnv::FnvHashMap::default(),
        };

        let mut headers = true;

        let mut data_temp = String::new();

        this.data.insert(Type::Raw, data.to_string());

        for line in data.lines() {
            if headers {
                if !line.is_empty() {
                    if &line[..1] == " " {
                        if let Some(prev_line) = this.headers.last_mut() {
                            prev_line.push_str("\r\n");
                            prev_line.push_str(line);
                            continue;
                        }
                    }
                    this.headers.push(line.to_string());
                } else {
                    headers = false;
                }
            } else {
                if !data_temp.is_empty() {
                    data_temp.push_str("\r\n");
                }
                data_temp.push_str(line);
            }
        }
        this.data.insert(Type::Text, data_temp);

        // Extract Date
        if let Some(date_str) = this.get_header_content("Date", false) {
            let local_date = DateTime::parse_from_rfc2822(date_str.as_str()).ok();
            if let Some(local_date) = local_date {
                this.date = local_date.with_timezone(&Utc);
            }
        }

        // Extract Subject
        if let Some(subject) = this.get_header_content("Subject", false) {
            this.subject = subject;
        }

        this
    }

    pub fn get_id(&self) -> Ulid {
        self.id
    }

    pub fn from(&self) -> &String {
        &self.from
    }
    pub fn to(&self) -> &Vec<String> {
        &self.to
    }

    /**
     * Retrieve mail date, either from the Date header or if not present from the reception time
     */
    pub fn get_date(&self) -> DateTime<Utc> {
        self.date
    }

    pub fn get_subject(&self) -> &String {
        &self.subject
    }

    /**
     * Retrieve the content in text format
     */
    pub fn get_text(&self) -> Option<&String> {
        self.data.get(&Type::Text)
    }

    /**
     * Retrieve the content in html format
     */
    pub fn get_html(&self) -> Option<&String> {
        if let Some(html) = self.data.get(&Type::Html) {
            Some(html)
        } else {
            None
        }
    }

    pub fn get_header_content(&self, key: &str, raw: bool) -> Option<String> {
        let key = format!("{}: ", key);
        let key = key.as_str();
        let key_len = key.len();

        for header in &self.headers {
            if header.len() > key_len && &header[..key_len] == key {
                let content = &header[key_len..];
                return Some(if raw {
                    content.to_string()
                } else {
                    decode_string(content)
                });
            }
        }
        None
    }

    pub fn get_headers(&self) -> &Vec<String> {
        self.headers.as_ref()
    }

    pub fn get_size(&self) -> usize {
        self.data.get(&Type::Raw).unwrap().len()
    }

    pub fn get_data(&self, type_: &Type) -> Option<&String> {
        self.data.get(type_)
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
