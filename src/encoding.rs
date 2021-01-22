use encoding::{label::encoding_from_whatwg_label, DecoderTrap};
use lazy_static::lazy_static;
use regex::{Captures, Regex};

lazy_static! {
    static ref RE_REMOVE_SPACE: Regex =
        Regex::new(r"(?P<first>=\?[^?]+\?.\?.+?\?=)[ \t\r\n]+(?P<second>=\?[^?]+\?.\?.+?\?=)")
            .unwrap();
    static ref RE_GENERAL: Regex =
        Regex::new(r"(=\?(?P<charset>[^?]+)\?(?P<encoding>.)\?(?P<encoded_text>.+?)\?=)").unwrap();
    static ref RE_QUOTE: regex::bytes::Regex =
        regex::bytes::Regex::new("\x3D([\x30-\x39\x41-\x46]{2})").unwrap();
    static ref HEX_BYTE: fnv::FnvHashMap<String, u8> = {
        let mut m: fnv::FnvHashMap<String, u8> = fnv::FnvHashMap::default();
        // Insert with 0 leading
        for i in 0x00..=0xFF {
            m.insert(format!("{:02X}", i), i);
        }
        m
    };
}

pub fn decode_string(string: &str) -> String {
    let mut string: String = string.to_string();
    while RE_REMOVE_SPACE.is_match(&string) {
        string = RE_REMOVE_SPACE
            .replace_all(&string, |caps: &Captures| {
                format!("{}{}", &caps["first"], &caps["second"])
            })
            .to_string();
    }
    RE_GENERAL.replace_all(&string, rfc2047_decode).to_string()
}

fn rfc2047_decode(caps: &Captures) -> String {
    if let Some(dec) = encoding_from_whatwg_label(&caps["charset"].to_lowercase()) {
        let text: Vec<u8> = match caps["encoding"].to_lowercase().as_str() {
            "b" => base64::decode(&caps["encoded_text"])
                .unwrap_or_else(|_| b"/!\\ Invalid Base64 encoding /!\\".to_vec()),
            "q" => {
                let text: String = caps["encoded_text"].replace("_", "\u{20}");
                RE_QUOTE.replace_all(text.as_bytes(), replace_byte).to_vec()
            }
            _ => panic!("Illegal encoding"),
        };
        dec.decode(&text, DecoderTrap::Strict).unwrap_or_default()
    } else {
        format!(
            "=?{}?{}?{}?=",
            &caps["charset"], &caps["encoding"], &caps["encoded_text"]
        )
    }
}

fn replace_byte(caps: &regex::bytes::Captures) -> Vec<u8> {
    let index: String = String::from_utf8((&caps[1]).to_vec()).unwrap();
    vec![HEX_BYTE[&index]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_string_literal() {
        let text: &str = "Subject: This is a simple literal string";
        let a: String = decode_string(text);

        assert_eq!(a, text);
    }

    #[test]
    fn decode_string_unknown_charset() {
        let text: &str = "Subject: =?invalid-charset?B?This is a simple literal string?=";
        let a: String = decode_string(text);

        assert_eq!(a, text);
    }

    #[test]
    fn decode_string_base64_encoded() {
        let text:&str = "Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?= =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=";
        let a: String = decode_string(text);

        assert_eq!(
            a,
            "Subject: If you can read this you understand the example."
        );
    }

    #[test]
    fn decode_string_quoted_encoded() {
        let text: &str = "From: =?ISO-8859-1?Q?Patrik_F=E4ltstr=F6m?= <paf@nada.kth.se>";
        let a: String = decode_string(text);

        assert_eq!(a, "From: Patrik Fältström <paf@nada.kth.se>");
    }

    #[test]
    fn hex_decoding() {
        assert_eq!(HEX_BYTE[&"00".to_string()], 0);
        assert_eq!(HEX_BYTE[&"10".to_string()], 0x10);
        assert_eq!(HEX_BYTE[&"FF".to_string()], 255);
    }
}
