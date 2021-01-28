use std::borrow::Cow;

use log::debug;
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
    debug!("content_len: {:?}", content.len());

    // Build the Response
    let response: ResponseBuilder = Response::builder(StatusCode::Ok)
        // specify the mime type
        .content_type(mime)
        // then the file length
        .header(headers::CONTENT_LENGTH, content.len().to_string());
    // If compression enabled, add the header to response
    let response: ResponseBuilder = if compressed {
        debug! {"using deflate compression output"};
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
