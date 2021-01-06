use core::future::Future;
use std::{
    fmt,
    time::{Duration, Instant},
};

use async_std::{net::SocketAddr, task};

// Spawn a new async task, waiting it completion,
// it display it's status at the end: Success or Error
pub fn spawn_task_and_swallow_log_errors<F>(task_name: String, fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = crate::Result<()>> + Send + 'static,
{
    task::Builder::new()
        .name(task_name.clone())
        .spawn(async move { log_errors(task_name, fut).await.unwrap_or_default() })
        .unwrap()
}

// Log Success or Error of the future completion
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
        let peer = if let Some(addr) = self.peer_addr {
            addr.to_string()
        } else {
            "Unknown".to_string()
        };
        let local = if let Some(addr) = self.local_addr {
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

#[allow(unused)]
pub fn wrap(text: &str, column: usize) -> String {
    let mut pos = 0;

    let mut res = String::new();

    while pos < text.len() {
        let start = pos;
        pos += column;
        if pos > text.len() {
            res.push_str("\r\n");
            res.push_str(text.get(start..).unwrap());
            break;
        }

        if !res.ends_with("\r\n") {
            res.push_str("\r\n");
        }
        let slice = text.get(start..pos).unwrap_or("");
        if let Some(i) = slice.find("\r\n") {
            pos = start + i + 2;
            res.push_str(slice.get(0..(i + 2)).unwrap());
        } else {
            while text.get((pos - 1)..pos) != Some(" ") {
                pos -= 1;
            }
            res.push_str(text.get(start..pos).unwrap());
        }
    }

    res.get(2..).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap() {
        let txt = wrap(
            "CHAPTER I\r\nDown the Rabbit-Hole\r\n\r\nAlice was beginning to get very tired of sitting by her sister on the bank, and of having nothing to do: once or twice she had peeped into the book her sister was reading, but it had no pictures or conversations in it, `and what is the use of a book,' thought Alice `without pictures or conversation?'\r\n\r\nSo she was considering in her own mind (as well as she could, for the hot day made her feel very sleepy and stupid), whether the pleasure of making a daisy-chain would be worth the trouble of getting up and picking the daisies, when suddenly a White Rabbit with pink eyes ran close by her.\r\n\r\n  -- ALICE'S ADVENTURES IN WONDERLAND by Lewis Carroll",
            72,
        );
        assert_eq!(txt, "CHAPTER I\r\nDown the Rabbit-Hole\r\n\r\nAlice was beginning to get very tired of sitting by her sister on the \r\nbank, and of having nothing to do: once or twice she had peeped into \r\nthe book her sister was reading, but it had no pictures or \r\nconversations in it, `and what is the use of a book,' thought Alice \r\n`without pictures or conversation?'\r\n\r\nSo she was considering in her own mind (as well as she could, for the \r\nhot day made her feel very sleepy and stupid), whether the pleasure of \r\nmaking a daisy-chain would be worth the trouble of getting up and \r\npicking the daisies, when suddenly a White Rabbit with pink eyes ran \r\nclose by her.\r\n\r\n  -- ALICE'S ADVENTURES IN WONDERLAND by Lewis Carroll");
    }
}
