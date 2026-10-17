//! Single sign-on through the server's OpenID Connect provider, as a native app (RFC 8252):
//! authorization code flow with PKCE (RFC 7636, S256), a random `state` and `nonce`, and a
//! one-shot loopback redirect listener on `http://127.0.0.1:<port>/callback`.
//!
//! [`begin`] binds the listener and builds the authorization URL the user's browser opens;
//! [`Pending::wait_for_code`] accepts the browser's redirect, checks `state`, answers with a
//! small page and returns the authorization code, which the caller exchanges with the server
//! (`POST /auth/oidc/exchange`) together with the verifier. The verifier, code and tokens
//! never appear in messages or `Debug` output.
use crate::CloudError;
use crate::proto::OidcInfo;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the browser has to complete the sign-in.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Path of the loopback redirect.
pub const CALLBACK_PATH: &str = "/callback";
/// Longest request head read from the browser.
const MAX_HEAD: usize = 16 * 1024;

/// Unpadded base64url (RFC 4648 §5).
pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// The S256 PKCE challenge of `verifier`: base64url(SHA-256(verifier)).
pub fn pkce_challenge(verifier: &str) -> String {
    b64url(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref())
}

/// `n` bytes from the operating system's CSPRNG, base64url-encoded.
fn random_token(n: usize) -> Result<String, CloudError> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; n];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| CloudError::Invalid("the system random generator failed".into()))?;
    Ok(b64url(&buf))
}

/// Percent-encode a query component (RFC 3986 unreserved characters kept).
pub fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode a query component (`+` is a space); None for malformed escapes or non-UTF-8.
pub fn query_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The `key=value` pairs of a query string (malformed pairs dropped).
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Some((query_decode(k)?, query_decode(v)?))
        })
        .collect()
}

/// A sign-in waiting for the browser's redirect.
pub struct Pending {
    pub(crate) verifier: String,
    pub(crate) state: String,
    pub(crate) nonce: String,
    pub(crate) redirect_uri: String,
    listener: TcpListener,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("redirect_uri", &self.redirect_uri)
            .field("verifier", &"<redacted>")
            .finish()
    }
}

/// Bind the loopback listener and build the authorization URL for `info`.
pub fn begin(info: &OidcInfo) -> Result<(Pending, String), CloudError> {
    let endpoint = info.authorization_endpoint.trim();
    let lower = endpoint.to_ascii_lowercase();
    // The browser opens this URL: only http(s), and https unless it is a loopback provider.
    let loopback = lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://[::1]");
    if !(lower.starts_with("https://") || loopback)
        || endpoint
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CloudError::Protocol(format!(
            "the server advertises an unusable sign-in address {endpoint:.200}"
        )));
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let verifier = random_token(32)?; // 43 characters
    let state = random_token(16)?;
    let nonce = random_token(16)?;
    let scopes = if info.scopes.split_whitespace().any(|s| s == "openid") {
        info.scopes.clone()
    } else {
        format!("openid {}", info.scopes).trim().to_string()
    };
    let params = [
        ("response_type", "code"),
        ("client_id", info.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", scopes.as_str()),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge", &pkce_challenge(&verifier)),
        ("code_challenge_method", "S256"),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", query_encode(v)))
    .collect::<Vec<_>>()
    .join("&");
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{endpoint}{sep}{params}");
    Ok((
        Pending {
            verifier,
            state,
            nonce,
            redirect_uri,
            listener,
        },
        url,
    ))
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>R-CAD</title>\
    <body style=\"font-family:sans-serif;margin:3em\"><h1>Signed in</h1>\
    <p>You can close this window and return to R-CAD.</p>";
const PAGE_FAILED: &str = "<!doctype html><meta charset=utf-8><title>R-CAD</title>\
    <body style=\"font-family:sans-serif;margin:3em\"><h1>Sign-in failed</h1>\
    <p>Return to R-CAD for details. You can close this window.</p>";

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// The request target of an HTTP request head (`GET <target> HTTP/1.1`).
fn read_target(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() + n > MAX_HEAD {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut parts = text.lines().next()?.split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Some(target.to_string()),
        _ => None,
    }
}

impl Pending {
    /// The exact redirect URI sent in the authorization request.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Wait for the browser's redirect to the callback and return the authorization code.
    /// Other paths (a browser's favicon request) get 404 and the wait goes on. Fails on a
    /// provider error (`error=`), a wrong `state`, `cancel` being set, or `timeout`.
    pub fn wait_for_code(
        &self,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<String, CloudError> {
        self.listener.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CloudError::Invalid("sign-in cancelled".into()));
            }
            if Instant::now() >= deadline {
                return Err(CloudError::Invalid(
                    "sign-in timed out waiting for the browser".into(),
                ));
            }
            let mut stream = match self.listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let _ = stream.set_nonblocking(false);
            let Some(target) = read_target(&mut stream) else {
                respond(&mut stream, "400 Bad Request", PAGE_FAILED);
                continue;
            };
            let (path, query) = target.split_once('?').unwrap_or((&target, ""));
            if path != CALLBACK_PATH {
                respond(&mut stream, "404 Not Found", "");
                continue;
            }
            let params = parse_query(query);
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
            if let Some(err) = get("error") {
                respond(&mut stream, "200 OK", PAGE_FAILED);
                let detail = get("error_description")
                    .map(|d| format!(" ({})", d.chars().take(200).collect::<String>()))
                    .unwrap_or_default();
                return Err(CloudError::Invalid(format!(
                    "the identity provider refused the sign-in: {}{detail}",
                    err.chars().take(64).collect::<String>()
                )));
            }
            if get("state") != Some(self.state.as_str()) {
                respond(&mut stream, "400 Bad Request", PAGE_FAILED);
                return Err(CloudError::Invalid(
                    "the sign-in answer did not match this request (state mismatch); try again"
                        .into(),
                ));
            }
            let Some(code) = get("code").filter(|c| !c.is_empty()) else {
                respond(&mut stream, "400 Bad Request", PAGE_FAILED);
                return Err(CloudError::Invalid(
                    "the identity provider sent no authorization code".into(),
                ));
            };
            respond(&mut stream, "200 OK", PAGE_OK);
            return Ok(code.to_string());
        }
    }
}

/// Open `url` (an `https://` or loopback `http://` address) in the user's default browser:
/// `xdg-open` on Linux and the BSDs, `open` on macOS, the URL protocol handler on Windows
/// (`rundll32`, which unlike `cmd /c start` does not reinterpret `&` in the URL). The
/// command is spawned and not waited for.
pub fn open_browser(url: &str) -> std::io::Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "only http(s) addresses are opened",
        ));
    }
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_7636_example() {
        // RFC 7636, Appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn query_components_round_trip() {
        let s = "openid profile&x=1/é?";
        assert_eq!(query_decode(&query_encode(s)).as_deref(), Some(s));
        assert_eq!(
            parse_query("code=a%2Fb&state=x+y&bad=%zz&flag"),
            vec![
                ("code".to_string(), "a/b".to_string()),
                ("state".to_string(), "x y".to_string()),
                ("flag".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn the_authorization_url_carries_every_parameter() {
        let info = OidcInfo {
            display_name: "Pocket ID".into(),
            authorization_endpoint: "https://id.example/authorize?tenant=a".into(),
            client_id: "rcad desktop".into(),
            scopes: "email profile".into(),
        };
        let (p, url) = begin(&info).unwrap();
        assert!(
            url.starts_with("https://id.example/authorize?tenant=a&"),
            "{url}"
        );
        let q = parse_query(url.split_once('?').unwrap().1);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("response_type").as_deref(), Some("code"));
        assert_eq!(get("client_id").as_deref(), Some("rcad desktop"));
        assert_eq!(get("scope").as_deref(), Some("openid email profile"));
        assert_eq!(get("redirect_uri").as_deref(), Some(p.redirect_uri()));
        assert!(p.redirect_uri().starts_with("http://127.0.0.1:"));
        assert_eq!(get("state"), Some(p.state.clone()));
        assert_eq!(get("nonce"), Some(p.nonce.clone()));
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("code_challenge"), Some(pkce_challenge(&p.verifier)));
        assert!((43..=128).contains(&p.verifier.len()));
        assert!(!format!("{p:?}").contains(&p.verifier));
        // Fresh randomness every time.
        let (p2, _) = begin(&info).unwrap();
        assert_ne!(p.verifier, p2.verifier);
        assert_ne!(p.state, p2.state);
        // Unusable endpoints are refused.
        for bad in [
            "javascript:alert(1)",
            "http://id.example/a",
            "file:///etc/passwd",
        ] {
            let info = OidcInfo {
                authorization_endpoint: bad.into(),
                ..info.clone()
            };
            assert!(begin(&info).is_err(), "{bad}");
        }
    }
}
