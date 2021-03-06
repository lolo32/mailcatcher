use tide::{Request, Server};

use crate::{
    http::{sse_evt::SseEvt, State},
    mail::Mail,
};

/// Append the routes for creating fake emails, with prefix: `/fake`
pub fn append_route(app: &mut Server<State<SseEvt>>) {
    // Generate one fake mail
    let _route_fake = app.at("/fake").get(faking);
    // Generate :nb fake mails, 1 if not a number
    let _route_fake_nb = app.at("/fake/:nb").get(faking);
}

/// Generate `n` fake email
async fn faking(req: Request<State<SseEvt>>) -> tide::Result<String> {
    let nb: usize = req
        .param("nb")
        .map(|nb| nb.parse::<usize>().unwrap_or(1))
        .unwrap_or(1);

    for _ in 0..nb {
        let mail: Mail = Mail::fake();

        match req.state().new_fake_mail.send(mail).await {
            Ok(()) => log::debug!("New faked mail sent!"),
            Err(e) => log::debug!("New mail error: {:?}", e),
        }
    }

    Ok(format!("OK: {}", nb))
}
