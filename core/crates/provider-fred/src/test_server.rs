//! A tiny local HTTP/1.1 server for tests: serves canned responses and
//! records every request it receives. Never talks to the vendor.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One received request: the request target (`/path?query`) and headers
/// (names lower-cased).
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub target: String,
    pub headers: Vec<(String, String)>,
}

type Responder = Arc<dyn Fn(&str) -> (u16, String) + Send + Sync>;

pub(crate) struct TestServer {
    pub base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    connections: Arc<Mutex<usize>>,
}

impl TestServer {
    /// Starts a server; `respond` maps a request target to (status, body).
    pub async fn start(respond: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(Mutex::new(0));
        let respond: Responder = Arc::new(respond);
        let (reqs, conns) = (requests.clone(), connections.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                *conns.lock().unwrap() += 1;
                let (reqs, respond) = (reqs.clone(), respond.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let mut lines = text.split("\r\n");
                    let request_line = lines.next().unwrap_or_default();
                    let target = request_line.split_whitespace().nth(1).unwrap_or_default().to_owned();
                    let headers = lines
                        .take_while(|l| !l.is_empty())
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                        .collect();
                    let (status, body) = respond(&target);
                    reqs.lock().unwrap().push(Recorded { target, headers });
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self { base: format!("http://{addr}"), requests, connections }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    pub fn connections(&self) -> usize {
        *self.connections.lock().unwrap()
    }
}

/// Value of query parameter `name` in a request target, percent-decoded.
pub(crate) fn query_param(target: &str, name: &str) -> Option<String> {
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok()?;
    url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}
