use futures::StreamExt;
use log::{info, trace, warn};
use tide::{sse::Sender, Request};

use super::{
    sse_evt::{SseData, SseEvt},
    State,
};

/// Handle Server-Sent Events
pub async fn handle_sse(req: Request<State<SseEvt>>, sender: Sender) -> tide::Result<()> {
    // Retrieve the SSE stream notifications
    let mut sse_stream = req.state().sse_stream.clone();

    // Do for each event
    while let Some(mail_evt) = sse_stream.next().await {
        info!(
            "received new SSE notification, sending event to stream: {:?}",
            mail_evt
        );

        // Convert the Event to a data struct
        let data: SseData = mail_evt.into();
        trace!("data to send: {:?}", data);
        // Send the generated data
        let sent = sender.send(data.name, data.data.as_ref(), None).await;
        // Check the send result, exit of the SSE if any error, generally from a disconnection
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
