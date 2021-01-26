use core::future::Future;
use std::{
    fmt,
    time::{Duration, Instant},
};

use async_std::{net::SocketAddr, task};
use log::{error, info};

/// Spawn a new async task, waiting it completion,
/// it display it's status at the end: Success or Error
pub fn spawn_task_and_swallow_log_errors<F>(task_name: String, fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = crate::Result<()>> + Send + 'static,
{
    task::Builder::new()
        .name(task_name.clone())
        .spawn(async move { log_errors(task_name, fut).await.unwrap_or_default() })
        .unwrap()
}

/// Log Success or Error of the future completion
async fn log_errors<F, T, E>(task_name: String, fut: F) -> Option<T>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    match fut.await {
        Ok(r) => {
            info!("{} completes successfully.", task_name);
            Some(r)
        }
        Err(e) => {
            error!("Error in {}: {}", task_name, e);
            None
        }
    }
}

#[derive(Debug)]
pub struct ConnectionInfo {
    pub local_addr: Option<SocketAddr>,
    pub peer_addr: Option<SocketAddr>,
    pub connected_at: Instant,
}

impl ConnectionInfo {
    pub fn new(local_addr: Option<SocketAddr>, peer_addr: Option<SocketAddr>) -> Self {
        Self {
            local_addr,
            peer_addr,
            connected_at: Instant::now(),
        }
    }

    pub fn get_duration(&self) -> Duration {
        Instant::now() - self.connected_at
    }
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        ConnectionInfo::new(None, None)
    }
}

impl fmt::Display for ConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let peer: String = if let Some(addr) = self.peer_addr {
            addr.to_string()
        } else {
            "Unknown".to_string()
        };
        let local: String = if let Some(addr) = self.local_addr {
            addr.to_string()
        } else {
            "Unknown".to_string()
        };

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
