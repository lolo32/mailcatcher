use std::borrow::Cow;

use encoding::{label::encoding_from_whatwg_label, DecoderTrap};
use regex::{Captures, Regex};
use std::collections::HashMap;

lazy_static! {
    static ref RE_REMOVE_SPACE: Regex =
        Regex::new(r"(?P<first>=\?[^?]+\?.\?.+?\?=) +(?P<second>=\?[^?]+\?.\?.+?\?=)").unwrap();
    static ref RE_GENERAL: Regex =
        Regex::new(r"(=\?(?P<charset>[^?]+)\?(?P<encoding>.)\?(?P<encoded_text>.+?)\?=)").unwrap();
    static ref RE_QUOTE: regex::bytes::Regex =
        regex::bytes::Regex::new("\x3D([\x30-\x39\x41-\x46]{2})").unwrap();
    static ref HEX_BYTE: HashMap<String, u8> = {
        let mut m = HashMap::new();
        for i in 0..0xFF {
            m.insert(format!("{:X}", i), i);
        }
        m
    };
}

pub fn decode_string(string: &str) -> String {
    let mut string = string.to_string();
    while RE_REMOVE_SPACE.is_match(string.as_str()) {
        string = RE_REMOVE_SPACE.replace_all(string.as_str(), t).to_string();
    }
    RE_GENERAL
        .replace_all(string.as_str(), rfc2047_decode)
        .to_string()
}

fn t(caps: &Captures) -> String {
    format!("{}{}", &caps["first"], &caps["second"])
}

fn rfc2047_decode(caps: &Captures) -> String {
    if let Some(dec) = encoding_from_whatwg_label(&caps["charset"].to_lowercase()) {
        let text = rfc2047_decode_encoding(&caps["encoding"], &caps["encoded_text"]);
        return dec.decode(&text, DecoderTrap::Strict).unwrap_or_default();
    }

    format!(
        "=?{}?{}?{}?=",
        &caps["charset"], &caps["encoding"], &caps["encoded_text"]
    )
}

fn rfc2047_decode_encoding(encoding_: &str, text: &str) -> Vec<u8> {
    match encoding_.to_lowercase().as_str() {
        "b" => {
            base64::decode(text).unwrap_or_else(|_| b"/!\\ Invalid Base64 encoding /!\\".to_vec())
        }
        "q" => {
            let text = text.replace("_", "\u{20}");
            RE_QUOTE.replace_all(text.as_bytes(), replace_byte).to_vec()
        }
        _ => panic!("Illegal encoding"),
    }
}

fn replace_byte(caps: &regex::bytes::Captures) -> Vec<u8> {
    let index = String::from_utf8((&caps[1]).to_vec()).unwrap();
    vec![HEX_BYTE[&index]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_string_literal() {
        let text = "Subject: This is a simple literal string";
        let a = decode_string(text);

        assert_eq!(a, text);
    }

    #[test]
    fn decode_string_unknown_charset() {
        let text = "Subject: =?invalid-charset?B?This is a simple literal string?=";
        let a = decode_string(text);

        assert_eq!(a, text);
    }

    #[test]
    fn decode_string_base64_encoded() {
        let text = "Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?= =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=";
        let a = decode_string(text);

        assert_eq!(
            a,
            "Subject: If you can read this you understand the example."
        );
    }

    #[test]
    fn decode_string_quoted_encoded() {
        let text = "From: =?ISO-8859-1?Q?Patrik_F=E4ltstr=F6m?= <paf@nada.kth.se>";
        let a = decode_string(text);

        assert_eq!(a, "From: Patrik Fältström <paf@nada.kth.se>");
    }
}
