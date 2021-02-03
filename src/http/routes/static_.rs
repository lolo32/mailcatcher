use tide::{http::mime, Request, Server};

use crate::http::{asset::send, sse_evt::SseEvt, State};

/// Append route for files inside the Asset directory
pub async fn append_route(app: &mut Server<State<SseEvt>>) {
    let _route_html = app.at("/").get(|req: Request<State<SseEvt>>| async move {
        send(&req.into(), "home.html", mime::HTML)
    });
    let _route_js = app
        .at("/hyperapp.js")
        .get(|req: Request<State<SseEvt>>| async move {
            send(&req.into(), "hyperapp.js", mime::JAVASCRIPT)
        });
    let _route_css = app
        .at("/w3.css")
        .get(|req: Request<State<SseEvt>>| async move { send(&req.into(), "w3.css", mime::CSS) });
}
