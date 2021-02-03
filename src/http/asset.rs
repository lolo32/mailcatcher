use std::borrow::Cow;

use mailcatcher_derive::AssetEmbed;
use tide::{
    http::{headers, Mime, Request},
    Error, Response, ResponseBuilder, StatusCode,
};

#[derive(AssetEmbed)]
#[folder = "asset/"]
pub struct Asset;

/// Generate a Response based on the name of the asset and the mime type
#[allow(clippy::indexing_slicing)]
pub fn send(req: &Request, name: &str, mime: Mime) -> tide::Result<Response> {
    // Look if the response can be compressed in deflate, or not
    let compressed: bool = req
        .header(headers::ACCEPT_ENCODING)
        .map_or(false, |header_value| {
            header_value[0].as_str().contains("deflate")
        });
    // Retrieve the asset, either integrated during release compilation, or read from filesystem if it's debug build
    let content: Cow<[u8]> = Asset::get(name)
        .ok_or_else(|| Error::from_str(StatusCode::NotFound, "Unknown filename"))?;
    // If compression if available, ...
    let content: Cow<[u8]> = if compressed {
        // ... do nothing
        content
    } else {
        // ... uncompress the file content
        let uncompressed: Vec<u8> =
            miniz_oxide::inflate::decompress_to_vec(&content[..]).map_err(|e| {
                Error::from_str(
                    StatusCode::InternalServerError,
                    format!("Inflate error: {:?}", e),
                )
            })?;
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
    Ok(response.body(&*content).build())
}

#[cfg(test)]
mod tests {
    use std::{env, fs::metadata, path::Path};

    use tide::http::{mime, Method, Request};

    use super::*;

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn compressed() {
        let mut request = Request::new(Method::Get, "http://localhost/");
        let _ = request.insert_header(headers::ACCEPT_ENCODING, "gzip, deflate");
        let response = send(&request, "home.html", mime::HTML).expect("data");

        let header_type = response
            .header(headers::CONTENT_TYPE)
            .expect("Content-Type header present");
        assert_eq!(header_type[0].to_string(), mime::HTML.to_string());

        let header_encoding = response
            .header(headers::CONTENT_ENCODING)
            .expect("Content-Encoding header present");
        assert_eq!(header_encoding[0], "deflate");

        let meta = metadata(
            Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| {
                env::current_dir()
                    .expect("cwd")
                    .to_str()
                    .expect("cwd to str")
                    .to_owned()
            }))
            .join("asset")
            .join("home.html"),
        )
        .expect("metadata");

        let header_length = response
            .header(headers::CONTENT_LENGTH)
            .expect("Content-Length header present");
        let size: u64 = header_length[0].to_string().parse().expect("u64");
        assert!(size < meta.len());

        let header_modified = response
            .header(headers::LAST_MODIFIED)
            .expect("Last-Modified header present");
        assert!(header_modified[0].to_string().len() > 1);
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn uncompressed() {
        let request = Request::new(Method::Get, "http://localhost/");
        let response = send(&request, "home.html", mime::HTML).expect("data");

        let header_type = response
            .header(headers::CONTENT_TYPE)
            .expect("Content-Type header present");
        assert_eq!(header_type[0].to_string(), mime::HTML.to_string());

        let header_encoding = response.header(headers::CONTENT_ENCODING);
        assert!(header_encoding.is_none());

        let meta = metadata(
            Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| {
                env::current_dir()
                    .expect("cwd")
                    .to_str()
                    .expect("cwd to str")
                    .to_owned()
            }))
            .join("asset")
            .join("home.html"),
        )
        .expect("metadata");

        let header_length = response
            .header(headers::CONTENT_LENGTH)
            .expect("Content-Length header present");
        let size: u64 = header_length[0].to_string().parse().expect("u64");
        assert_eq!(size, meta.len());

        let header_modified = response
            .header(headers::LAST_MODIFIED)
            .expect("Last-Modified header present");
        assert!(header_modified[0].to_string().len() > 1);
    }
}
