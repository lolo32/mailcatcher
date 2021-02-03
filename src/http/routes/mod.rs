use tide::Server;

use super::{sse, sse_evt::SseEvt, State};

#[cfg(feature = "faking")]
/// Create fake email
mod faking;
/// Get mails or mail informations
mod get_mails;
/// Removing mail(s)
mod remove;
/// Files in the asset directory
mod static_;

/// Initialise the routes
pub async fn init(state: State<SseEvt>) -> crate::Result<Server<State<SseEvt>>> {
    let mut app: Server<State<SseEvt>> = tide::with_state(state);

    // static files
    static_::append_route(&mut app).await;
    // Retrieve mails information
    get_mails::append_route(&mut app);
    // Remove mail(s)
    remove::append_route(&mut app);
    // SSE stream
    let _route = app.at("/sse").get(tide::sse::endpoint(sse::handle));

    #[cfg(feature = "faking")]
    faking::append_route(&mut app);

    Ok(app)
}
