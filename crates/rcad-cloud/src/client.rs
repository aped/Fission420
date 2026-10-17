//! Blocking HTTP client for the `rcad_cloud_proto` routes.
//!
//! One method per route. Bodies are JSON except blob uploads and version content, which are
//! raw bytes; content downloads stream to a file while hashing. Failures map to:
//!
//! * [`CloudError::Api`]: a non-2xx answer with the server's [`ApiError`] body (a non-2xx
//!   answer without one gets a code derived from the HTTP status).
//! * [`CloudError::Offline`]: DNS, connect, TLS, timeout or connection reset, and 502/503/504
//!   without an `ApiError` body (a reverse proxy in front of a stopped server).
//! * [`CloudError::Protocol`]: a 2xx body that does not parse, a redirect (the account must
//!   use the final URL), or downloaded bytes whose BLAKE3 hash does not match.
//!
//! TLS uses rustls with the operating system's certificate verifier. Plain `http://` URLs are
//! for trusted networks (the password and token then travel unencrypted; see docs/CLOUD.md):
//! signing in over `http://` to a host [`plain_http_is_local`] does not recognise needs the
//! user's explicit consent ([`crate::service::Request::SignIn`]'s `allow_insecure_http`).
//!
//! Downloads are bounded: content streams stop at the size the server declared for the
//! version (and never exceed [`DEFAULT_MAX_DOWNLOAD_BYTES`] or the server's advertised
//! `max_blob_bytes`), so a hostile server cannot fill the disk or memory.
use crate::proto::{self, ApiError, ErrorCode};
use crate::{CloudError, fsutil};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Connect timeout of every request.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole-request timeout of JSON requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Whole-request timeout of blob uploads and content downloads.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Largest JSON response body accepted.
const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;
/// Largest download when the server's `max_blob_bytes` is not known, and the cap of the
/// in-memory downloads (`shared_content`, `download_content_bytes`).
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// The eight-byte signature every PNG file starts with.
pub const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Check a thumbnail before upload or after download: a PNG (by signature) of at most
/// [`proto::MAX_THUMBNAIL_BYTES`].
pub fn check_thumbnail(png: &[u8]) -> Result<(), CloudError> {
    if png.len() as u64 > proto::MAX_THUMBNAIL_BYTES {
        return Err(CloudError::Invalid(format!(
            "the thumbnail is {} bytes, more than the {}-byte limit",
            png.len(),
            proto::MAX_THUMBNAIL_BYTES
        )));
    }
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(CloudError::Invalid(
            "the thumbnail is not a PNG image".into(),
        ));
    }
    Ok(())
}

/// Shown wherever a sign-in over plain `http://` needs consent.
pub const INSECURE_HTTP_WARNING: &str = "This server address uses http://, so your password, \
     your access token and your documents travel unencrypted and anyone on the network path \
     can read or change them. Use https:// unless this is a network you trust.";

/// The host part of a normalized server URL, without port or IPv6 brackets.
fn url_host(server_url: &str) -> &str {
    let rest = server_url.split_once("://").map_or(server_url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or("");
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    match authority.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    }
}

/// Whether plain `http://` to `server_url` stays on a local or private network: a loopback
/// name or address, a private, link-local or carrier-grade NAT (100.64.0.0/10, used by
/// Tailscale) IPv4 address, a unique-local or link-local IPv6 address, or a name under
/// `.local`, `.lan`, `.internal`, `.home.arpa`, `.localhost` or `.ts.net`. Anything else
/// (including single-label names, which a search domain can resolve publicly) is not.
pub fn plain_http_is_local(server_url: &str) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    fn v4_local(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (b & 0xC0) == 64)
    }
    let host = url_host(server_url)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => v4_local(ip),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(v4_local)
        }
        Err(_) => {
            host == "localhost"
                || [
                    ".localhost",
                    ".local",
                    ".lan",
                    ".internal",
                    ".home.arpa",
                    ".ts.net",
                ]
                .iter()
                .any(|suffix| host.ends_with(suffix) && host.len() > suffix.len())
        }
    }
}

/// Whether signing in to `server_url` (normalized) needs consent to an unencrypted
/// connection: plain `http://` to a host that is not [`plain_http_is_local`].
pub fn needs_insecure_consent(server_url: &str) -> bool {
    server_url
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
        && !plain_http_is_local(server_url)
}

/// Validate and normalize a server URL: `http://` or `https://`, a host, no query or
/// fragment, no trailing slash. A bare host gets `https://`.
pub fn normalize_server_url(input: &str) -> Result<String, CloudError> {
    let s = input.trim();
    let s = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let (scheme, rest) = s.split_once("://").unwrap_or(("", ""));
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(CloudError::Protocol(format!(
            "server URL must start with https:// or http://: {input}"
        )));
    }
    if rest.contains(['?', '#']) || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CloudError::Protocol(format!("invalid server URL: {input}")));
    }
    let rest = rest.trim_end_matches('/');
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains('@') {
        return Err(CloudError::Protocol(format!("invalid server URL: {input}")));
    }
    Ok(format!("{scheme}://{rest}"))
}

/// Percent-encode one path segment (server ids are opaque and must not add `/` or `..`).
fn seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        // Unreachable after encoding ('.' is encoded), but never emit an empty segment.
        out = "%2E".repeat(out.len().max(1));
    }
    out
}

fn agent(global: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(global))
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(crate::generator().replace(' ', "/"))
        .tls_config(tls)
        .build()
        .into()
}

fn map_transport(e: ureq::Error) -> CloudError {
    use ureq::Error as E;
    match e {
        E::BadUri(m) => CloudError::Protocol(format!("invalid URL: {m}")),
        E::BodyExceedsLimit(n) => CloudError::Protocol(format!("response larger than {n} bytes")),
        E::TooManyRedirects | E::RedirectFailed => {
            CloudError::Protocol("the server redirected; use the final server URL".into())
        }
        other => CloudError::Offline(other.to_string()),
    }
}

fn code_for_status(status: u16) -> ErrorCode {
    match status {
        400 => ErrorCode::BadRequest,
        401 => ErrorCode::Unauthenticated,
        403 => ErrorCode::Forbidden,
        404 => ErrorCode::NotFound,
        409 => ErrorCode::AlreadyExists,
        410 => ErrorCode::Gone,
        413 => ErrorCode::TooLarge,
        422 => ErrorCode::HashMismatch,
        423 => ErrorCode::Locked,
        429 => ErrorCode::RateLimited,
        507 => ErrorCode::QuotaExceeded,
        500..=599 => ErrorCode::Internal,
        _ => ErrorCode::Other,
    }
}

/// Map a non-2xx response (status + body) to an error.
fn error_from(status: u16, body: &[u8], location: Option<String>) -> CloudError {
    if let Ok(api) = serde_json::from_slice::<ApiError>(body) {
        return CloudError::Api(api);
    }
    if (300..400).contains(&status) {
        let to = location.map(|l| format!(" to {l}")).unwrap_or_default();
        return CloudError::Protocol(format!(
            "the server redirected{to} (HTTP {status}); use the final server URL"
        ));
    }
    if matches!(status, 502..=504) {
        return CloudError::Offline(format!("HTTP {status} from a proxy in front of the server"));
    }
    let snippet: String = String::from_utf8_lossy(body).chars().take(200).collect();
    CloudError::Api(ApiError {
        code: code_for_status(status),
        message: format!("HTTP {status}: {}", snippet.trim()),
        head_version: None,
        lock: None,
    })
}

/// Blocking client for one server, optionally authenticated.
pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
    transfer: ureq::Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

type Resp = ureq::http::Response<ureq::Body>;

impl Client {
    /// A client for `server_url` (normalized with [`normalize_server_url`]).
    pub fn new(server_url: &str) -> Result<Client, CloudError> {
        Ok(Client {
            base: normalize_server_url(server_url)?,
            token: None,
            agent: agent(REQUEST_TIMEOUT),
            transfer: agent(TRANSFER_TIMEOUT),
        })
    }

    /// Authenticate later requests with `token`.
    pub fn with_token(mut self, token: impl Into<String>) -> Client {
        self.token = Some(token.into());
        self
    }

    /// The normalized server URL.
    pub fn server_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{path}", self.base, proto::API_PREFIX)
    }

    fn auth(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {t}"))
    }

    fn check(resp: Resp) -> Result<Resp, CloudError> {
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut resp = resp;
        let body = resp
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_vec()
            .unwrap_or_default();
        Err(error_from(status, &body, location))
    }

    /// Send a request with an optional JSON body and return the checked response.
    fn send(
        &self,
        agent: &ureq::Agent,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
    ) -> Result<Resp, CloudError> {
        let url = self.url(path);
        let mut b = ureq::http::Request::builder().method(method).uri(&url);
        if let Some(a) = self.auth() {
            b = b.header("Authorization", a);
        }
        b = b.header("Accept", "application/json");
        let empty: &[u8] = &[];
        let resp = match body {
            Some((ctype, bytes)) => {
                let req = b
                    .header("Content-Type", ctype)
                    .body(bytes)
                    .map_err(|e| CloudError::Protocol(format!("invalid request: {e}")))?;
                agent.run(req)
            }
            // POST/PUT/PATCH without a body still send `Content-Length: 0`.
            None if !matches!(method, "GET" | "DELETE") => {
                let req = b
                    .body(empty)
                    .map_err(|e| CloudError::Protocol(format!("invalid request: {e}")))?;
                agent.run(req)
            }
            None => {
                let req = b
                    .body(())
                    .map_err(|e| CloudError::Protocol(format!("invalid request: {e}")))?;
                agent.run(req)
            }
        }
        .map_err(map_transport)?;
        Self::check(resp)
    }

    fn json<T: DeserializeOwned>(mut resp: Resp) -> Result<T, CloudError> {
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_vec()
            .map_err(map_transport)?;
        serde_json::from_slice(&body)
            .map_err(|e| CloudError::Protocol(format!("unexpected response body: {e}")))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, CloudError> {
        Self::json(self.send(&self.agent, "GET", path, None)?)
    }

    fn with_body<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &B,
    ) -> Result<T, CloudError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| CloudError::Protocol(format!("cannot encode request: {e}")))?;
        Self::json(self.send(
            &self.agent,
            method,
            path,
            Some(("application/json", &bytes)),
        )?)
    }

    fn no_content(&self, method: &str, path: &str) -> Result<(), CloudError> {
        self.send(&self.agent, method, path, None).map(drop)
    }

    // -- Server and auth ------------------------------------------------------------------------

    pub fn info(&self) -> Result<proto::ServerInfo, CloudError> {
        self.get("/info")
    }

    pub fn login(&self, req: &proto::LoginRequest) -> Result<proto::LoginResponse, CloudError> {
        self.with_body("POST", "/auth/login", req)
    }

    pub fn logout(&self) -> Result<(), CloudError> {
        self.no_content("POST", "/auth/logout")
    }

    pub fn me(&self) -> Result<proto::Me, CloudError> {
        self.get("/me")
    }

    pub fn create_token(
        &self,
        req: &proto::CreateTokenRequest,
    ) -> Result<proto::CreateTokenResponse, CloudError> {
        self.with_body("POST", "/tokens", req)
    }

    pub fn tokens(&self) -> Result<Vec<proto::TokenInfo>, CloudError> {
        self.get("/tokens")
    }

    pub fn delete_token(&self, token_id: &str) -> Result<(), CloudError> {
        self.no_content("DELETE", &format!("/tokens/{}", seg(token_id)))
    }

    // -- Organizations --------------------------------------------------------------------------

    pub fn orgs(&self) -> Result<Vec<proto::OrgSummary>, CloudError> {
        self.get("/orgs")
    }

    pub fn create_org(
        &self,
        req: &proto::CreateOrgRequest,
    ) -> Result<proto::OrgSummary, CloudError> {
        self.with_body("POST", "/orgs", req)
    }

    pub fn org_members(&self, org: &str) -> Result<Vec<proto::OrgMember>, CloudError> {
        self.get(&format!("/orgs/{}/members", seg(org)))
    }

    pub fn set_org_member(
        &self,
        org: &str,
        req: &proto::SetOrgMemberRequest,
    ) -> Result<proto::OrgMember, CloudError> {
        self.with_body("PUT", &format!("/orgs/{}/members", seg(org)), req)
    }

    pub fn remove_org_member(&self, org: &str, username: &str) -> Result<(), CloudError> {
        self.no_content(
            "DELETE",
            &format!("/orgs/{}/members/{}", seg(org), seg(username)),
        )
    }

    pub fn org_usage(&self, org: &str) -> Result<proto::OrgUsage, CloudError> {
        self.get(&format!("/orgs/{}/usage", seg(org)))
    }

    pub fn org_activity(
        &self,
        org: &str,
        limit: Option<u32>,
    ) -> Result<Vec<proto::ActivityEntry>, CloudError> {
        let q = limit.map(|n| format!("?limit={n}")).unwrap_or_default();
        self.get(&format!("/orgs/{}/activity{q}", seg(org)))
    }

    // -- Blobs ----------------------------------------------------------------------------------

    pub fn missing_blobs(
        &self,
        org: &str,
        hashes: &[proto::BlobHash],
    ) -> Result<Vec<proto::BlobHash>, CloudError> {
        let req = proto::MissingBlobsRequest {
            hashes: hashes.to_vec(),
        };
        let r: proto::MissingBlobsResponse =
            self.with_body("POST", &format!("/orgs/{}/blobs/missing", seg(org)), &req)?;
        Ok(r.missing)
    }

    /// Upload `bytes` under `hash` (checked locally first; the server re-hashes).
    pub fn upload_blob(
        &self,
        org: &str,
        hash: &str,
        bytes: &[u8],
    ) -> Result<proto::BlobInfo, CloudError> {
        if !proto::is_blob_hash(hash) || crate::blob_hash(bytes) != hash {
            return Err(CloudError::Protocol(
                "blob bytes do not match their hash".into(),
            ));
        }
        let path = format!("/orgs/{}/blobs/{}", seg(org), seg(hash));
        Self::json(self.send(
            &self.transfer,
            "PUT",
            &path,
            Some(("application/octet-stream", bytes)),
        )?)
    }

    // -- Projects and folders -------------------------------------------------------------------

    pub fn projects(&self, org: &str) -> Result<Vec<proto::Project>, CloudError> {
        self.get(&format!("/orgs/{}/projects", seg(org)))
    }

    pub fn create_project(
        &self,
        org: &str,
        req: &proto::CreateProjectRequest,
    ) -> Result<proto::Project, CloudError> {
        self.with_body("POST", &format!("/orgs/{}/projects", seg(org)), req)
    }

    pub fn project(&self, project: &str) -> Result<proto::Project, CloudError> {
        self.get(&format!("/projects/{}", seg(project)))
    }

    pub fn update_project(
        &self,
        project: &str,
        req: &proto::UpdateProjectRequest,
    ) -> Result<proto::Project, CloudError> {
        self.with_body("PATCH", &format!("/projects/{}", seg(project)), req)
    }

    pub fn project_members(&self, project: &str) -> Result<Vec<proto::ProjectMember>, CloudError> {
        self.get(&format!("/projects/{}/members", seg(project)))
    }

    pub fn set_project_member(
        &self,
        project: &str,
        req: &proto::SetProjectMemberRequest,
    ) -> Result<proto::ProjectMember, CloudError> {
        self.with_body("PUT", &format!("/projects/{}/members", seg(project)), req)
    }

    pub fn remove_project_member(&self, project: &str, username: &str) -> Result<(), CloudError> {
        self.no_content(
            "DELETE",
            &format!("/projects/{}/members/{}", seg(project), seg(username)),
        )
    }

    pub fn tree(&self, project: &str) -> Result<proto::ProjectTree, CloudError> {
        self.get(&format!("/projects/{}/tree", seg(project)))
    }

    pub fn create_folder(
        &self,
        project: &str,
        req: &proto::CreateFolderRequest,
    ) -> Result<proto::Folder, CloudError> {
        self.with_body("POST", &format!("/projects/{}/folders", seg(project)), req)
    }

    pub fn update_folder(
        &self,
        folder: &str,
        req: &proto::UpdateFolderRequest,
    ) -> Result<proto::Folder, CloudError> {
        self.with_body("PATCH", &format!("/folders/{}", seg(folder)), req)
    }

    pub fn delete_folder(&self, folder: &str) -> Result<(), CloudError> {
        self.no_content("DELETE", &format!("/folders/{}", seg(folder)))
    }

    // -- Documents and versions -----------------------------------------------------------------

    pub fn create_document(
        &self,
        project: &str,
        req: &proto::CreateDocumentRequest,
    ) -> Result<proto::DocumentSummary, CloudError> {
        self.with_body(
            "POST",
            &format!("/projects/{}/documents", seg(project)),
            req,
        )
    }

    pub fn document(&self, doc: &str) -> Result<proto::DocumentDetail, CloudError> {
        self.get(&format!("/documents/{}", seg(doc)))
    }

    pub fn update_document(
        &self,
        doc: &str,
        req: &proto::UpdateDocumentRequest,
    ) -> Result<proto::DocumentSummary, CloudError> {
        self.with_body("PATCH", &format!("/documents/{}", seg(doc)), req)
    }

    /// Move to the trash.
    pub fn delete_document(&self, doc: &str) -> Result<(), CloudError> {
        self.no_content("DELETE", &format!("/documents/{}", seg(doc)))
    }

    /// Restore from the trash.
    pub fn restore_document(&self, doc: &str) -> Result<proto::DocumentSummary, CloudError> {
        Self::json(self.send(
            &self.agent,
            "POST",
            &format!("/documents/{}/restore", seg(doc)),
            None,
        )?)
    }

    pub fn create_version(
        &self,
        doc: &str,
        req: &proto::CreateVersionRequest,
    ) -> Result<proto::Version, CloudError> {
        self.with_body("POST", &format!("/documents/{}/versions", seg(doc)), req)
    }

    pub fn update_version(
        &self,
        doc: &str,
        number: u64,
        req: &proto::UpdateVersionRequest,
    ) -> Result<proto::Version, CloudError> {
        self.with_body(
            "PATCH",
            &format!("/documents/{}/versions/{number}", seg(doc)),
            req,
        )
    }

    pub fn promote(
        &self,
        doc: &str,
        number: u64,
        req: &proto::PromoteRequest,
    ) -> Result<proto::Version, CloudError> {
        self.with_body(
            "POST",
            &format!("/documents/{}/versions/{number}/promote", seg(doc)),
            req,
        )
    }

    /// Stream version `number`'s bytes into `dest` (replaced atomically). When `expected` is
    /// given, bytes whose BLAKE3 hash differs are rejected and `dest` is left unchanged. A
    /// body longer than `max_bytes` (the version's declared size) is cut off and rejected,
    /// and the partial file is deleted. Returns the hash and size of what was written.
    pub fn download_content(
        &self,
        doc: &str,
        number: u64,
        expected: Option<&str>,
        max_bytes: u64,
        dest: &Path,
    ) -> Result<(proto::BlobHash, u64), CloudError> {
        let path = format!("/documents/{}/versions/{number}/content", seg(doc));
        let resp = self.send(&self.transfer, "GET", &path, None)?;
        stream_to_file(resp, expected, max_bytes, dest)
    }

    /// Version `number`'s bytes in memory, checked against `expected` when given.
    pub fn download_content_bytes(
        &self,
        doc: &str,
        number: u64,
        expected: Option<&str>,
    ) -> Result<Vec<u8>, CloudError> {
        let path = format!("/documents/{}/versions/{number}/content", seg(doc));
        let resp = self.send(&self.transfer, "GET", &path, None)?;
        read_checked(resp, expected)
    }

    /// Version `number`'s PNG thumbnail, or None when it has none (404). The body is capped
    /// at [`proto::MAX_THUMBNAIL_BYTES`] and must carry the PNG signature.
    pub fn download_thumbnail(
        &self,
        doc: &str,
        number: u64,
    ) -> Result<Option<Vec<u8>>, CloudError> {
        let path = format!("/documents/{}/versions/{number}/thumbnail", seg(doc));
        let mut resp = match self.send(&self.agent, "GET", &path, None) {
            Ok(r) => r,
            Err(CloudError::Api(e)) if e.code == ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let png = resp
            .body_mut()
            .with_config()
            .limit(proto::MAX_THUMBNAIL_BYTES)
            .read_to_vec()
            .map_err(map_transport)?;
        check_thumbnail(&png)
            .map_err(|e| CloudError::Protocol(format!("server thumbnail: {e}")))?;
        Ok(Some(png))
    }

    // -- Locks ----------------------------------------------------------------------------------

    pub fn lock(&self, doc: &str) -> Result<proto::Lock, CloudError> {
        Self::json(self.send(
            &self.agent,
            "POST",
            &format!("/documents/{}/lock", seg(doc)),
            None,
        )?)
    }

    pub fn unlock(&self, doc: &str) -> Result<(), CloudError> {
        self.no_content("DELETE", &format!("/documents/{}/lock", seg(doc)))
    }

    // -- Comments -------------------------------------------------------------------------------

    pub fn comments(&self, doc: &str) -> Result<Vec<proto::Comment>, CloudError> {
        self.get(&format!("/documents/{}/comments", seg(doc)))
    }

    pub fn create_comment(
        &self,
        doc: &str,
        req: &proto::CreateCommentRequest,
    ) -> Result<proto::Comment, CloudError> {
        self.with_body("POST", &format!("/documents/{}/comments", seg(doc)), req)
    }

    pub fn update_comment(
        &self,
        comment: &str,
        req: &proto::UpdateCommentRequest,
    ) -> Result<proto::Comment, CloudError> {
        self.with_body("PATCH", &format!("/comments/{}", seg(comment)), req)
    }

    // -- Share links ----------------------------------------------------------------------------

    pub fn shares(&self, doc: &str) -> Result<Vec<proto::ShareLink>, CloudError> {
        self.get(&format!("/documents/{}/shares", seg(doc)))
    }

    pub fn create_share(
        &self,
        doc: &str,
        req: &proto::CreateShareRequest,
    ) -> Result<proto::CreateShareResponse, CloudError> {
        self.with_body("POST", &format!("/documents/{}/shares", seg(doc)), req)
    }

    pub fn delete_share(&self, share: &str) -> Result<(), CloudError> {
        self.no_content("DELETE", &format!("/shares/{}", seg(share)))
    }

    /// Public metadata of a share link (no authentication needed).
    pub fn shared(&self, token: &str) -> Result<proto::SharedDocument, CloudError> {
        self.get(&format!("/shared/{}", seg(token)))
    }

    /// A share link's document bytes, if the link allows downloading.
    pub fn shared_content(&self, token: &str) -> Result<Vec<u8>, CloudError> {
        let resp = self.send(
            &self.transfer,
            "GET",
            &format!("/shared/{}/content", seg(token)),
            None,
        )?;
        read_checked(resp, None)
    }
}

fn read_checked(mut resp: Resp, expected: Option<&str>) -> Result<Vec<u8>, CloudError> {
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(DEFAULT_MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(map_transport)?;
    if let Some(want) = expected {
        let got = crate::blob_hash(&bytes);
        if got != want {
            return Err(CloudError::Protocol(format!(
                "downloaded content hash {got} does not match {want}"
            )));
        }
    }
    Ok(bytes)
}

fn too_large(max_bytes: u64) -> CloudError {
    CloudError::Protocol(format!(
        "the server sent more than the {max_bytes} bytes it declared; download discarded"
    ))
}

fn stream_to_file(
    resp: Resp,
    expected: Option<&str>,
    max_bytes: u64,
    dest: &Path,
) -> Result<(proto::BlobHash, u64), CloudError> {
    let declared = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > max_bytes) {
        return Err(too_large(max_bytes));
    }
    fsutil::ensure_parent(dest)?;
    let tmp = fsutil::temp_path_for(dest);
    let result = (|| {
        let mut file = fsutil::create_temp(&tmp, false)?;
        let mut reader = resp.into_body().into_reader();
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CloudError::Offline(format!("download interrupted: {e}"))),
            };
            size += n as u64;
            if size > max_bytes {
                return Err(too_large(max_bytes));
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
        }
        file.sync_all()?;
        drop(file);
        let got = hasher.finalize().to_hex().to_string();
        if let Some(want) = expected
            && got != want
        {
            return Err(CloudError::Protocol(format!(
                "downloaded content hash {got} does not match {want}"
            )));
        }
        std::fs::rename(&tmp, dest)?;
        Ok((got, size))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            normalize_server_url("https://cad.example/").unwrap(),
            "https://cad.example"
        );
        assert_eq!(
            normalize_server_url(" cad.example:8443 ").unwrap(),
            "https://cad.example:8443"
        );
        assert_eq!(
            normalize_server_url("HTTP://10.0.0.2:8080").unwrap(),
            "http://10.0.0.2:8080"
        );
        assert_eq!(
            normalize_server_url("https://h.example/rcad//").unwrap(),
            "https://h.example/rcad"
        );
        for bad in [
            "ftp://x",
            "https://",
            "https://x?y=1",
            "https://u:p@x",
            "https://x y",
            "",
        ] {
            assert!(normalize_server_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn plain_http_hosts_are_classified() {
        for local in [
            "http://localhost:8080",
            "http://app.localhost",
            "http://127.0.0.1",
            "http://127.9.9.9:1/x",
            "http://10.0.0.2:8080",
            "http://172.16.0.1",
            "http://172.31.255.255",
            "http://192.168.1.10/rcad",
            "http://169.254.3.4",
            "http://100.64.0.1",
            "http://100.127.255.254:8443",
            "http://[::1]:8080",
            "http://[fd12:3456::1]",
            "http://[fe80::1]",
            "http://[::ffff:192.168.0.5]",
            "http://nas.local",
            "http://NAS.LOCAL.:80",
            "http://cad.lan",
            "http://cad.corp.internal",
            "http://cad.home.arpa",
            "http://box.tail1234.ts.net",
        ] {
            assert!(plain_http_is_local(local), "{local}");
            assert!(!needs_insecure_consent(local), "{local}");
        }
        for public in [
            "http://cad.example",
            "http://8.8.8.8",
            "http://172.32.0.1",
            "http://100.128.0.1",
            "http://100.63.255.255",
            "http://[2001:db8::1]",
            "http://[::ffff:8.8.8.8]",
            "http://nas",
            "http://local",
            "http://evil.local.example.com",
            "http://localhost.example.com",
            "http://ts.net",
            "http://192.168.1.10.nip.io",
        ] {
            assert!(!plain_http_is_local(public), "{public}");
            assert!(needs_insecure_consent(public), "{public}");
        }
        // https never needs consent, whatever the host.
        assert!(!needs_insecure_consent("https://cad.example"));
        assert!(!needs_insecure_consent("https://8.8.8.8"));
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(seg("doc_1-A~"), "doc_1-A~");
        assert_eq!(seg("../x"), "%2E%2E%2Fx");
        assert_eq!(seg("a b/c"), "a%20b%2Fc");
        assert_eq!(seg(""), "%2E");
    }

    #[test]
    fn error_bodies_map_to_api_errors() {
        let body = br#"{"code":"version_conflict","message":"moved","head_version":7}"#;
        match error_from(409, body, None) {
            CloudError::Api(e) => {
                assert_eq!(e.code, ErrorCode::VersionConflict);
                assert_eq!(e.head_version, Some(7));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            error_from(502, b"Bad gateway", None),
            CloudError::Offline(_)
        ));
        match error_from(404, b"<html>nope</html>", None) {
            CloudError::Api(e) => assert_eq!(e.code, ErrorCode::NotFound),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            error_from(301, b"", Some("https://x".into())),
            CloudError::Protocol(_)
        ));
    }

    #[test]
    fn debug_redacts_the_token() {
        let c = Client::new("https://cad.example")
            .unwrap()
            .with_token("rcs_topsecret");
        assert!(!format!("{c:?}").contains("topsecret"));
    }
}
