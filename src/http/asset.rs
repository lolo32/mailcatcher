use std::borrow::Cow;

use mailcatcher_derive::AssetEmbed;
use tide::{
    http::{headers, Mime, Request},
    Response, ResponseBuilder, StatusCode,
};

#[derive(AssetEmbed)]
#[folder = "asset/"]
pub struct Asset;

/// Generate a Response based on the name of the asset and the mime type
pub fn send(req: &Request, name: &str, mime: Mime) -> Response {
    // Look if the response can be compressed in deflate, or not
    let compressed: bool = req
        .header(headers::ACCEPT_ENCODING)
        .map_or(false, |header_value| {
            header_value
                .get(0)
                .expect("accept-encoding")
                .as_str()
                .contains("deflate")
        });
    // Retrieve the asset, either integrated during release compilation, or read from filesystem if it's debug build
    let content: Cow<[u8]> = Asset::get(name).expect("asset resource");
    // If compression if available, ...
    let content: Cow<[u8]> = if compressed {
        // ... do nothing
        content
    } else {
        // ... uncompress the file content
        let uncompressed: Vec<u8> =
            miniz_oxide::inflate::decompress_to_vec(&content[..]).expect("deflate invalid");
        Cow::Owned(uncompressed)
    };
    log::debug!("content_len: {:?}", content.len());

    // Build the Response
    let response: ResponseBuilder = Response::builder(StatusCode::Ok)
        // specify the mime type
        .content_type(mime)
        // then the file length
        .header(headers::CONTENT_LENGTH, content.len().to_string());
    // If compression enabled, add the header to response
    let response: ResponseBuilder = if compressed {
        log::debug! {"using deflate compression output"};
        response.header(headers::CONTENT_ENCODING, "deflate")
    } else {
        response
    };
    // If the last modified date is available, add the content to the header
    let response = match Asset::modif(name) {
        Some(modif) => response.header(headers::LAST_MODIFIED, modif),
        None => response,
    };

    // Return the Response with content
    response.body(&*content).build()
}

#[cfg(test)]
mod tests {
    use std::{env, fs::metadata, path::Path};

    use tide::http::{mime, Method, Request};

    use super::*;

    #[test]
    fn compressed() {
        let mut request = Request::new(Method::Get, "http://localhost/");
        let _ = request.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
        let res = send(&request, "home.html", mime::HTML);

        let header = res.header(headers::CONTENT_TYPE).unwrap();
        assert_eq!(header[0].to_string(), mime::HTML.to_string());

        let header = res.header(headers::CONTENT_ENCODING).unwrap();
        assert_eq!(header[0], "deflate");

        let meta = metadata(
            Path::new(
                &env::var("CARGO_MANIFEST_DIR")
                    .unwrap_or_else(|_| env::current_dir().unwrap().to_str().unwrap().to_owned()),
            )
            .join("asset")
            .join("home.html"),
        )
        .unwrap();

        let header = res.header(headers::CONTENT_LENGTH).unwrap();
        let size: u64 = header[0].to_string().parse().unwrap();
        assert!(size < meta.len());

        let header = res.header(headers::LAST_MODIFIED).unwrap();
        assert!(header[0].to_string().len() > 1);
    }

    #[test]
    fn uncompressed() {
        let request = Request::new(Method::Get, "http://localhost/");
        let res = send(&request, "home.html", mime::HTML);

        let header = res.header(headers::CONTENT_TYPE).unwrap();
        assert_eq!(header[0].to_string(), mime::HTML.to_string());

        let header = res.header(headers::CONTENT_ENCODING);
        assert!(header.is_none());

        let meta = metadata(
            Path::new(
                &env::var("CARGO_MANIFEST_DIR")
                    .unwrap_or_else(|_| env::current_dir().unwrap().to_str().unwrap().to_owned()),
            )
            .join("asset")
            .join("home.html"),
        )
        .unwrap();

        let header = res.header(headers::CONTENT_LENGTH).unwrap();
        let size: u64 = header[0].to_string().parse().unwrap();
        assert_eq!(size, meta.len());

        let header = res.header(headers::LAST_MODIFIED).unwrap();
        assert!(header[0].to_string().len() > 1);
    }
}
