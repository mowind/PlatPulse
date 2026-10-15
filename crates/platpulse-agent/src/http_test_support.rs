//! Test-only HTTP/1.1 server that counts TCP connections and requests.
//!
//! Issue #244 is about Agent→Server connection reuse, which is only observable
//! where the two sides meet: a keep-alive-aware server counts one accept per
//! TCP connection and every request that rides on it. Asserting on the client's
//! internals instead would couple the tests to reqwest's pool.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct Observations {
    connections: usize,
    requests: usize,
    user_agents: Vec<String>,
}

/// A loopback keep-alive server that answers every request with the same body.
pub(crate) struct KeepAliveServer {
    base_url: String,
    observations: Arc<Mutex<Observations>>,
}

impl KeepAliveServer {
    /// Start a server that answers immediately.
    pub(crate) async fn start(body: String) -> Self {
        Self::start_with_delay(body, Duration::ZERO).await
    }

    /// Start a server that answers after `delay`.
    pub(crate) async fn start_with_delay(body: String, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the loopback listener binds");
        let base_url = format!("http://{}", listener.local_addr().expect("a bound address"));
        let observations = Arc::new(Mutex::new(Observations::default()));
        let observed = Arc::clone(&observations);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                observed.lock().expect("the observation lock").connections += 1;
                let observed = Arc::clone(&observed);
                let body = body.clone();
                tokio::spawn(async move { serve(socket, &body, delay, &observed).await });
            }
        });
        Self {
            base_url,
            observations,
        }
    }

    /// Absolute URL for `path` on this server.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Accepted TCP connections so far.
    pub(crate) fn connection_count(&self) -> usize {
        self.observations
            .lock()
            .expect("the observation lock")
            .connections
    }

    /// Requests answered so far, across every connection.
    pub(crate) fn request_count(&self) -> usize {
        self.observations
            .lock()
            .expect("the observation lock")
            .requests
    }

    /// `User-Agent` values seen so far, in request order.
    pub(crate) fn user_agents(&self) -> Vec<String> {
        self.observations
            .lock()
            .expect("the observation lock")
            .user_agents
            .clone()
    }
}

/// Serve one connection until the client closes it. Requests carry no body, so
/// a request ends at the first empty line and the connection stays open.
async fn serve(mut socket: TcpStream, body: &str, delay: Duration, observed: &Mutex<Observations>) {
    let mut buffered = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let head_end = loop {
            if let Some(end) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(count) => buffered.extend_from_slice(&chunk[..count]),
            }
        };
        let head = String::from_utf8_lossy(&buffered[..head_end]).to_lowercase();
        buffered.drain(..head_end);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        {
            let mut state = observed.lock().expect("the observation lock");
            state.requests += 1;
            if let Some(agent) = head
                .lines()
                .find_map(|line| line.strip_prefix("user-agent: "))
            {
                state.user_agents.push(agent.trim().to_owned());
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        if socket.write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}
