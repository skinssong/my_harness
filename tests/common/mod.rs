#![allow(dead_code)] // compiled into each test binary; each uses a subset

//! A stub provider on localhost.
//!
//! It records the bytes it was sent and answers with what the test canned, so a
//! test can check both directions of the boundary without a key or a network.
//! `serve_many` takes a script of replies, one per request, which is what the
//! loop needs.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use harness::config::Config;

pub fn response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub fn config_at(addr: SocketAddr, key: &str) -> Config {
    let entries = [
        ("LLM_BASE_URL".to_string(), format!("http://{addr}/v1")),
        ("LLM_API_KEY".to_string(), key.to_string()),
        ("LLM_MODEL".to_string(), "m".to_string()),
    ];
    Config::from_lookup(move |name| {
        entries
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    })
    .unwrap()
}

/// Everything one request carried: headers and body as one string.
async fn read_request(socket: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    let header_end = loop {
        let read = socket.read(&mut chunk).await.unwrap();
        if read == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };

    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let declared = headers
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    while buf.len() < header_end + declared {
        let read = socket.read(&mut chunk).await.unwrap();
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }

    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    format!("{headers}\n{body}")
}

/// Accept exactly one connection, reply with `reply`, hand back what was sent.
pub async fn serve_once(reply: String) -> (SocketAddr, oneshot::Receiver<String>) {
    let (addr, rx) = serve_many(vec![reply]).await;
    let (tx, once) = oneshot::channel();
    tokio::spawn(async move {
        if let Ok(mut seen) = rx.await {
            if !seen.is_empty() {
                let _ = tx.send(seen.remove(0));
            }
        }
    });
    (addr, once)
}

/// Accept as many connections as there are replies, in order.
pub async fn serve_many(replies: Vec<String>) -> (SocketAddr, oneshot::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let mut seen = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            seen.push(read_request(&mut socket).await);
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
        }
        let _ = tx.send(seen);
    });

    (addr, rx)
}

/// A workspace directory of its own, removed when the test finishes.
pub struct TempWorkspace {
    pub path: std::path::PathBuf,
}

/// Two tests can ask for a workspace in the same nanosecond; a counter cannot
/// be asked twice.
static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

impl TempWorkspace {
    pub fn new(name: &str) -> Self {
        let seq = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "harness-{name}-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
