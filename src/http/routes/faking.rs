use log::debug;
use tide::{Request, Server};

use crate::{
    http::{sse_evt::SseEvt, State},
    mail::Mail,
};

#[allow(clippy::shadow_unrelated)]
pub fn append_route(app: &mut Server<State<SseEvt>>) {
    // Generate one fake mail
    let _route = app.at("/fake").get(faking);
    // Generate :nb fake mails, 1 if not a number
    let _route = app.at("/fake/:nb").get(faking);
}

async fn faking(req: Request<State<SseEvt>>) -> tide::Result<String> {
    let nb: usize = req
        .param("nb")
        .map(|nb| nb.parse::<usize>().unwrap_or(1))
        .unwrap_or(1);

    for _ in 0..nb {
        let mail: Mail = Mail::fake();

        match req.state().new_fake_mail.send(mail).await {
            Ok(()) => debug!("New faked mail sent!"),
            Err(e) => debug!("New mail error: {:?}", e),
        }
    }

    Ok(format!("OK: {}", nb))
}
