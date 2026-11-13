//! A one-shot HTTP listener on 127.0.0.1 that captures the OAuth redirect.
//!
//! Strava has no device-code flow, so the authorization code comes back as a browser redirect.
//! We bind a socket, hand Strava `http://localhost:<port>/callback` as the redirect URI, serve
//! exactly one request, and shut down.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The result of the single request the OAuth redirect delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectResult {
    Code { code: String, scopes: Vec<String> },
    Denied(String),
    TimedOut,
    Failed(String),
}

pub struct LoopbackServer {
    listener: TcpListener,
}

impl LoopbackServer {
    /// Binds before the browser opens, so a busy port is reported up front.
    pub async fn bind(port: u16) -> Result<Self, String> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match TcpListener::bind(addr).await {
            Ok(listener) => Ok(Self { listener }),
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                Err(format!("port {port} is already in use"))
            }
            Err(e) => Err(format!("could not listen on port {port}: {e}")),
        }
    }

    pub fn port(&self) -> u16 {
        self.listener.local_addr().map_or(0, |a| a.port())
    }

    /// Waits for the redirect, accepting a code only if it carries `state`.
    pub async fn await_redirect(self, wait: Duration, state: &str) -> RedirectResult {
        let accepted = tokio::time::timeout(wait, self.listener.accept()).await;
        let mut stream = match accepted {
            Err(_) => return RedirectResult::TimedOut,
            Ok(Err(e)) => return RedirectResult::Failed(format!("accept failed: {e}")),
            Ok(Ok((stream, _))) => stream,
        };

        let mut buf = vec![0u8; 8192];
        let read = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await
        {
            Ok(Ok(n)) if n > 0 => n,
            _ => return RedirectResult::Failed("empty request".into()),
        };
        let request = String::from_utf8_lossy(&buf[..read]);
        let result = parse_request(&request, state);

        let message = match &result {
            RedirectResult::Code { .. } => {
                "Authorized. You can close this tab and return to the terminal."
            }
            RedirectResult::Denied(_) => "Authorization was declined. You can close this tab.",
            _ => "Something went wrong. You can close this tab and try again.",
        };
        let body = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>cli-walker</title></head>\n\
             <body style=\"font-family:system-ui,sans-serif;padding:3rem;text-align:center\">\n\
             <h2>cli-walker &times; Strava</h2><p>{message}</p></body></html>"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        result
    }
}

/// Pulls the query parameters out of `GET /callback?state=...&code=...&scope=... HTTP/1.1`.
/// A code arriving without the `state` this login sent is refused, since it did not come
/// from this login.
fn parse_request(request: &str, expected_state: &str) -> RedirectResult {
    let Some(line) = request.lines().next() else {
        return RedirectResult::Failed("no request line".into());
    };
    let Some(target) = line.split(' ').nth(1) else {
        return RedirectResult::Failed("malformed request line".into());
    };
    let query = target.split_once('?').map_or("", |(_, q)| q);
    if query.is_empty() {
        return RedirectResult::Failed("redirect carried no parameters".into());
    }

    let mut code = None;
    let mut state = None;
    let mut scopes = Vec::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = url_decode(value);
        match key {
            "error" => return RedirectResult::Denied(value),
            "code" => code = Some(value),
            "state" => state = Some(value),
            "scope" => {
                scopes = value
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
            }
            _ => {}
        }
    }
    match code {
        Some(_) if state.as_deref() != Some(expected_state) => {
            RedirectResult::Failed("redirect did not come from this login (state mismatch)".into())
        }
        Some(code) => RedirectResult::Code { code, scopes },
        None => RedirectResult::Failed("redirect carried no code".into()),
    }
}

fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_authorization_redirect() {
        let request = "GET /callback?state=s1&code=abc123&scope=read,activity:write,activity:read_all HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(
            parse_request(request, "s1"),
            RedirectResult::Code {
                code: "abc123".into(),
                scopes: vec![
                    "read".into(),
                    "activity:write".into(),
                    "activity:read_all".into()
                ],
            }
        );
    }

    #[test]
    fn reports_a_declined_authorization() {
        assert_eq!(
            parse_request(
                "GET /callback?state=s1&error=access_denied HTTP/1.1\r\n",
                "s1"
            ),
            RedirectResult::Denied("access_denied".into())
        );
    }

    #[test]
    fn rejects_requests_without_a_code() {
        assert!(matches!(
            parse_request("GET /favicon.ico HTTP/1.1\r\n", "s1"),
            RedirectResult::Failed(_)
        ));
        assert!(matches!(
            parse_request("GET /callback?scope=read HTTP/1.1\r\n", "s1"),
            RedirectResult::Failed(_)
        ));
    }

    #[test]
    fn refuses_a_code_without_this_logins_state() {
        for request in [
            "GET /callback?code=abc&scope=activity:write HTTP/1.1\r\n",
            "GET /callback?state=&code=abc&scope=activity:write HTTP/1.1\r\n",
            "GET /callback?state=other&code=abc&scope=activity:write HTTP/1.1\r\n",
        ] {
            assert!(
                matches!(parse_request(request, "s1"), RedirectResult::Failed(_)),
                "{request}"
            );
        }
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(url_decode("activity%3Awrite%2Cread"), "activity:write,read");
        assert_eq!(url_decode("a+b"), "a b");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
        assert_eq!(url_decode("caf%C3%A9"), "café");
    }

    #[tokio::test]
    async fn serves_one_redirect_over_tcp() {
        let server = LoopbackServer::bind(0).await.unwrap();
        let port = server.port();
        let waiting = tokio::spawn(server.await_redirect(Duration::from_secs(5), "s1"));

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /callback?state=s1&code=xyz&scope=activity:write HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorized."));
        assert_eq!(
            waiting.await.unwrap(),
            RedirectResult::Code {
                code: "xyz".into(),
                scopes: vec!["activity:write".into()]
            }
        );
    }

    #[tokio::test]
    async fn times_out_without_a_redirect() {
        let server = LoopbackServer::bind(0).await.unwrap();
        assert_eq!(
            server.await_redirect(Duration::from_millis(20), "s1").await,
            RedirectResult::TimedOut
        );
    }

    #[tokio::test]
    async fn reports_a_busy_port() {
        let first = LoopbackServer::bind(0).await.unwrap();
        let err = LoopbackServer::bind(first.port()).await.err().unwrap();
        assert!(err.contains("already in use"), "{err}");
    }
}
