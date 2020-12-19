use tide::{sse::Sender, Request};

use super::{sse_evt::SseData, State};

pub async fn handle_sse(req: Request<State>, sender: Sender) -> Result<(), tide::Error> {
    use futures::StreamExt;

    let mut sse_stream = req.state().sse_stream.clone();

    while let Some(mail_evt) = sse_stream.next().await {
        info!(
            "received new SSE notification, sending event to stream: {:?}",
            mail_evt
        );

        let data = SseData::from(mail_evt);
        trace!("data to send: {:?}", data);
        let sent = sender
            .send(data.name.as_ref(), data.data.as_ref(), data.id)
            .await;
        if sent.is_err() {
            warn!("Err, disconnected: {:?}", sent);
            break;
        } else {
            trace!("### Server-Sent Events sent");
        }
    }
    info!("### Exit /sse");
    Ok(())
}
