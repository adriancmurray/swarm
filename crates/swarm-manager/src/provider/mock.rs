//! Network-free HTTP mock servers for provider tests.
//!
//! Each helper binds a `std::net::TcpListener` on `127.0.0.1:0` (an OS-chosen
//! ephemeral port) and serves exactly one request on a detached thread. The
//! server fully **drains the request body** — it reads headers, parses
//! `Content-Length`, then keeps reading until that many body bytes have
//! arrived — *before* writing the canned response. Replying early (then
//! closing) can race the client mid-send and surface as a broken-pipe /
//! `EINVAL`, intermittently; draining first avoids that class of flake.
//!
//! Test-only; never compiled into the shipping crate.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// A captured HTTP request, parsed enough to assert on in tests.
pub(crate) struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl CapturedRequest {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

/// Read a full HTTP/1.1 request (headers + Content-Length body) from a stream.
fn read_full_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];

    // Read until we have the full header block.
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk).expect("read request headers");
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    // Drain the body: anything already buffered past the headers, plus the
    // rest of Content-Length bytes.
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).expect("read request body");
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }

    CapturedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: &str,
) {
    let mut response = format!(
        "HTTP/1.1 {status} Test\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(body);
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Serve a single canned response with the given status/headers/body.
/// Returns the `http://127.0.0.1:<port>` base URL to point a provider at.
pub(crate) async fn single_response_server(
    status: u16,
    headers: &[(&str, &str)],
    body: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().expect("mock server addr");
    let body = body.to_string();
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept mock request");
        let _ = read_full_request(&mut stream);
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        write_response(&mut stream, status, &header_refs, &body);
    });

    format!("http://{addr}")
}

/// Serve a single `200 OK` JSON response and capture the inbound request.
/// The `Receiver` yields the parsed `CapturedRequest` once the request lands.
pub(crate) async fn request_capture_server(body: &str) -> (String, Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind capture server");
    let addr = listener.local_addr().expect("capture server addr");
    let body = body.to_string();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept capture request");
        let captured = read_full_request(&mut stream);
        write_response(
            &mut stream,
            200,
            &[("content-type", "application/json")],
            &body,
        );
        let _ = tx.send(captured);
    });

    (format!("http://{addr}"), rx)
}
