use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::encoding::decode_string;

#[derive(Hash, Eq, PartialEq)]
enum Type {
    Text,
    Html,
    Image,
    Pdf,
}

pub struct Mail {
    from: String,
    to: Vec<String>,
    subject: String,
    date: DateTime<Utc>,
    headers: Vec<String>,
    data: HashMap<Type, (Option<String>, String)>,
}

impl Mail {
    pub fn new(from: String, to: Vec<String>, data: String) -> Self {
        let mut this = Self {
            from,
            to,
            subject: "(No subject)".to_string(),
            date: Utc::now(),
            headers: Vec::default(),
            data: HashMap::default(),
        };

        let mut headers = true;

        let mut data_temp = String::new();

        for line in data.lines() {
            if headers {
                if line != "" {
                    if &line[..1] == " " {
                        if let Some(prev_line) = this.headers.last_mut() {
                            prev_line.push(' ');
                            prev_line.push_str(line.trim_start());
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
        this.data.insert(Type::Text, (None, data_temp));

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
        if let Some((_, text)) = self.data.get(&Type::Text) {
            Some(text)
        } else {
            self.get_html()
        }
    }

    /**
     * Retrieve the content in html format
     */
    pub fn get_html(&self) -> Option<&String> {
        if let Some((_, html)) = self.data.get(&Type::Html) {
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
                    decode_string(content).to_string()
                });
            }
        }
        None
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
        let mail = Mail::new(
            "from@example.com".to_string(),
            vec!["to@example.com".to_string()],
            DATA_SIMPLE.to_string(),
        );

        assert_eq!(mail.headers.len(), 6);
        assert_eq!(mail.from, "from@example.com");
        assert_eq!(mail.to.len(), 1);
        assert_eq!(mail.to.get(0), Some(&"to@example.com".to_string()));
        assert!(mail.data.contains_key(&Type::Text));
        assert_eq!(
            mail.data.get(&Type::Text),
            Some(&(None, "This is a test mailing\r\n\r\n".to_string()))
        );
    }

    #[test]
    fn test_getting_datetime() {
        let mail = Mail::new(String::default(), Vec::default(), DATA_SIMPLE.to_string());

        let date = mail.get_date();
        assert_eq!(date.timezone(), Utc);

        let dt = Utc.ymd(2020, 11, 22).and_hms(0, 58, 23);
        assert_eq!(date, dt);
    }

    #[test]
    fn get_text() {
        let mail = Mail::new(String::default(), Vec::default(), DATA_SIMPLE.to_string());

        let text = mail.get_text();

        assert!(text.is_some());
        assert_eq!(text.unwrap(), &"This is a test mailing\r\n\r\n".to_string());

        assert!(mail.get_html().is_none());
    }
}
