use core::future::Future;
use std::{
    fmt, io,
    time::{Duration, Instant},
};

use async_std::{net::SocketAddr, task};

/// Spawn a new async task, waiting it completion,
/// it display it's status at the end: Success or Error
pub fn spawn_task_and_swallow_log_errors<F>(
    task_name: String,
    fut: F,
) -> io::Result<task::JoinHandle<()>>
where
    F: Future<Output = crate::Result<()>> + Send + 'static,
{
    task::Builder::new()
        .name(task_name.clone())
        .spawn(async move { log_errors(task_name, fut).await.unwrap_or_default() })
}

/// Log Success or Error of the future completion
async fn log_errors<F, T, E>(task_name: String, fut: F) -> Option<T>
where
    F: Future<Output = Result<T, E>> + Send,
    E: std::fmt::Display,
{
    match fut.await {
        Ok(r) => {
            log::info!("{} completes successfully.", task_name);
            Some(r)
        }
        Err(e) => {
            log::error!("Error in {}: {}", task_name, e);
            None
        }
    }
}

/// Connection information, used primarily in SMTP
#[derive(Debug)]
pub struct ConnectionInfo {
    /// host address
    pub local_addr: Option<SocketAddr>,
    /// remote address
    pub peer_addr: Option<SocketAddr>,
    /// obscure connection timestamp for duration
    pub connected_at: Instant,
}

impl ConnectionInfo {
    /// Instantiate a new connection information
    pub fn new(local_addr: Option<SocketAddr>, peer_addr: Option<SocketAddr>) -> Self {
        Self {
            local_addr,
            peer_addr,
            connected_at: Instant::now(),
        }
    }

    /// Get connection duration
    pub fn get_duration(&self) -> Duration {
        Instant::now() - self.connected_at
    }
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl fmt::Display for ConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let peer: String = self
            .peer_addr
            .map_or_else(|| "Unknown".to_owned(), |addr| addr.to_string());
        let local: String = self
            .local_addr
            .map_or_else(|| "Unknown".to_owned(), |addr| addr.to_string());

        f.write_str(
            format!(
                "Connection from peer {} to local {} established {:?} ago.",
                peer,
                local,
                self.get_duration()
            )
            .as_str(),
        )
    }
}
