use futures::StreamExt;
use tide::{sse::Sender, Request};

use super::{
    sse_evt::{SseData, SseEvt},
    State,
};

/// Handle Server-Sent Events
pub async fn handle(req: Request<State<SseEvt>>, sender: Sender) -> tide::Result<()> {
    // Retrieve the SSE stream notifications
    let mut sse_stream = req.state().sse_stream.clone();

    // Do for each event
    'outer: while let Some(mail_evt) = sse_stream.next().await {
        log::info!(
            "received new SSE notification, sending event to stream: {:?}",
            mail_evt
        );

        // Convert the Event to a data struct
        let data: SseData = mail_evt.into();
        log::trace!("data to send: {:?}", data);
        // Send the generated data
        match sender.send(data.name, data.data.as_ref(), None).await {
            Ok(..) => {
                log::trace!("### Server-Sent Events sent");
            }
            // Check the send result, exit of the SSE if any error, generally from a disconnection
            Err(e) => {
                log::warn!("Err, disconnected: {:?}", e);
                break 'outer;
            }
        }
    }
    log::info!("### Exit /sse");
    Ok(())
}
