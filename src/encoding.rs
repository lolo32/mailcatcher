use encoding::{label::encoding_from_whatwg_label, DecoderTrap};
use lazy_static::lazy_static;
use regex::{Captures, Regex};

lazy_static! {
    static ref RE_REMOVE_SPACE: Regex =
        Regex::new(r"(?P<first>=\?[^?]+\?.\?.+?\?=)[ \t\r\n]+(?P<second>=\?[^?]+\?.\?.+?\?=)")
            .expect("re remove space");
    static ref RE_GENERAL: Regex =
        Regex::new(r"(=\?(?P<charset>[^?]+)\?(?P<encoding>.)\?(?P<encoded_text>.+?)\?=)").expect("re general");
    static ref RE_QUOTE: regex::bytes::Regex =
        regex::bytes::Regex::new("\x3D([\x30-\x39\x41-\x46]{2})").expect("re quote");
    static ref HEX_BYTE: fnv::FnvHashMap<String, u8> = {
        let mut m: fnv::FnvHashMap<String, u8> = fnv::FnvHashMap::default();
        // Insert with 0 leading
        for i in 0x00..=0xFF {
            let _ = m.insert(format!("{:02X}", i), i);
        }
        m
    };
}

/// decode base64/quote encoded string and remove space separator between
/// encoded string to literal values
#[allow(clippy::indexing_slicing)]
pub fn decode_string(string: &str) -> String {
    let mut string: String = string.to_owned();
    // Remove all spaces between encoded string
    while RE_REMOVE_SPACE.is_match(&string) {
        string = RE_REMOVE_SPACE
            .replace_all(&string, |caps: &Captures| {
                format!("{}{}", &caps["first"], &caps["second"])
            })
            .to_string();
    }
    RE_GENERAL.replace_all(&string, rfc2047_decode).to_string()
}

/// Decode base64/quote encoded string
#[allow(clippy::indexing_slicing)]
fn rfc2047_decode(caps: &Captures) -> String {
    // Search if the encoding is known
    if let Some(dec) = encoding_from_whatwg_label(&caps["charset"].to_lowercase()) {
        // check charset used (only support "b" or "q")
        let text: Vec<u8> = match caps["encoding"].to_lowercase().as_str() {
            // Base 64
            "b" => base64::decode(&caps["encoded_text"])
                .unwrap_or_else(|_| b"/!\\ Invalid Base64 encoding /!\\".to_vec()),
            // Quote
            "q" => {
                let text: String = caps["encoded_text"].replace("_", "\u{20}");
                RE_QUOTE.replace_all(text.as_bytes(), replace_byte).to_vec()
            }
            // Anything else
            #[allow(clippy::panic)]
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

/// Quote replacing function, convert any hexadecimal value to it's representation
#[allow(clippy::indexing_slicing)]
fn replace_byte(caps: &regex::bytes::Captures) -> Vec<u8> {
    let index = String::from_utf8((&caps[1]).to_vec()).expect("invalid utf8 sequence");
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
        let text: &str = "Subject: =?ISO-8859-1?B?SWYgeW91IGNhbiByZWFkIHRoaXMgeW8=?=\n =?ISO-8859-2?B?dSB1bmRlcnN0YW5kIHRoZSBleGFtcGxlLg==?=";
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

        assert_eq!(a, "From: Patrik F\u{e4}ltstr\u{f6}m <paf@nada.kth.se>");
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn hex_decoding() {
        assert_eq!(HEX_BYTE[&"00".to_owned()], 0);
        assert_eq!(HEX_BYTE[&"10".to_owned()], 0x10);
        assert_eq!(HEX_BYTE[&"FF".to_owned()], 255);
    }
}
