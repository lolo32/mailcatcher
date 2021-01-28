use tide::{http::mime, Request, Server};

use crate::http::{asset::send as send_asset, sse_evt::SseEvt, State};

pub async fn append_route(app: &mut Server<State<SseEvt>>) {
    let _route_html = app.at("/").get(|req: Request<State<SseEvt>>| async move {
        Ok(send_asset(&req.into(), "home.html", mime::HTML))
    });
    let _route_js = app
        .at("/hyperapp.js")
        .get(|req: Request<State<SseEvt>>| async move {
            Ok(send_asset(&req.into(), "hyperapp.js", mime::JAVASCRIPT))
        });
    let _route_css = app
        .at("/w3.css")
        .get(|req: Request<State<SseEvt>>| async move {
            Ok(send_asset(&req.into(), "w3.css", mime::CSS))
        });
}
