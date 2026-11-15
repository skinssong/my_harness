//! A stub provider on localhost.
//!
//! It records the bytes it was sent and answers with what the test canned, so a
//! test can check both directions of the boundary without a key or a network.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub fn response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Accept exactly one connection, reply with `reply`, and hand back everything
/// the client sent: headers and body as one string.
pub async fn serve_once(reply: String) -> (SocketAddr, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
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
        let _ = tx.send(format!("{headers}\n{body}"));
        socket.write_all(reply.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });

    (addr, rx)
}
