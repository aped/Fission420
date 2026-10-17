//! Wire types of the optional R-CAD sync server protocol, version 1.
//!
//! R-CAD is complete without a server. A server adds shared projects, document versions,
//! edit locks, share links and comments for an organization. This crate is the whole
//! contract between the desktop client and any server: plain serde types, JSON over HTTPS,
//! no I/O. It is MIT OR Apache-2.0 like the rest of R-CAD, so anyone can implement a server.
//!
//! # Conventions
//!
//! * Every route is under [`API_PREFIX`]. Request and response bodies are JSON, except blob
//!   uploads and downloads, which are raw bytes (`application/octet-stream`).
//! * Authenticated requests carry `Authorization: Bearer <token>`. Tokens are opaque.
//! * Errors are a non-2xx status with an [`ApiError`] body. Clients branch on
//!   [`ApiError::code`], never on the message text.
//! * Ids are opaque strings chosen by the server. Version numbers are dense per document,
//!   starting at 1.
//! * Times are Unix seconds (UTC).
//! * Blobs are addressed by the lowercase hex BLAKE3 hash of their bytes ([`BlobHash`]).
//!
//! # Routes (method, path, request body → response body)
//!
//! ```text
//! GET    /api/v1/info                                   → ServerInfo            (no auth)
//! POST   /api/v1/auth/login            LoginRequest     → LoginResponse         (no auth)
//! POST   /api/v1/auth/oidc/exchange    OidcExchangeRequest → LoginResponse      (no auth; when ServerInfo.oidc is set)
//! POST   /api/v1/auth/logout                            → 204
//! GET    /api/v1/me                                     → Me
//! POST   /api/v1/tokens                CreateTokenRequest → CreateTokenResponse (personal access token)
//! GET    /api/v1/tokens                                 → Vec<TokenInfo>
//! DELETE /api/v1/tokens/{token_id}                      → 204
//!
//! GET    /api/v1/orgs                                   → Vec<OrgSummary>       (orgs I belong to)
//! POST   /api/v1/orgs                  CreateOrgRequest → OrgSummary            (if the server allows)
//! GET    /api/v1/orgs/{org}/members                     → Vec<OrgMember>
//! PUT    /api/v1/orgs/{org}/members    SetOrgMemberRequest → OrgMember          (org admin)
//! DELETE /api/v1/orgs/{org}/members/{username}          → 204                   (org admin)
//! GET    /api/v1/orgs/{org}/usage                       → OrgUsage
//! GET    /api/v1/orgs/{org}/activity?limit=N            → Vec<ActivityEntry>    (org admin)
//! POST   /api/v1/orgs/{org}/blobs/missing  MissingBlobsRequest → MissingBlobsResponse
//! PUT    /api/v1/orgs/{org}/blobs/{hash}   raw bytes    → BlobInfo              (server re-hashes)
//!
//! GET    /api/v1/orgs/{org}/projects                    → Vec<Project>          (visible to me)
//! POST   /api/v1/orgs/{org}/projects   CreateProjectRequest → Project           (org member)
//! GET    /api/v1/projects/{project}                     → Project
//! PATCH  /api/v1/projects/{project}    UpdateProjectRequest → Project           (project admin)
//! GET    /api/v1/projects/{project}/members             → Vec<ProjectMember>
//! PUT    /api/v1/projects/{project}/members SetProjectMemberRequest → ProjectMember (project admin)
//! DELETE /api/v1/projects/{project}/members/{username}  → 204                   (project admin)
//! GET    /api/v1/projects/{project}/tree                → ProjectTree
//! GET    /api/v1/projects/{project}/activity?limit=N    → Vec<ActivityEntry>    (viewer; newest first, at most 200)
//! POST   /api/v1/projects/{project}/folders CreateFolderRequest → Folder        (editor)
//! PATCH  /api/v1/folders/{folder}      UpdateFolderRequest → Folder             (editor)
//! DELETE /api/v1/folders/{folder}                       → 204 (must be empty)   (editor)
//!
//! POST   /api/v1/projects/{project}/documents CreateDocumentRequest → DocumentSummary (editor)
//! GET    /api/v1/documents/{doc}                        → DocumentDetail
//! PATCH  /api/v1/documents/{doc}       UpdateDocumentRequest → DocumentSummary  (editor)
//! DELETE /api/v1/documents/{doc}                        → 204 (to trash)        (editor)
//! POST   /api/v1/documents/{doc}/restore                → DocumentSummary       (editor)
//! POST   /api/v1/documents/{doc}/versions CreateVersionRequest → Version        (editor)
//! PATCH  /api/v1/documents/{doc}/versions/{n} UpdateVersionRequest → Version    (editor)
//! POST   /api/v1/documents/{doc}/versions/{n}/promote PromoteRequest → Version  (editor)
//! GET    /api/v1/documents/{doc}/versions/{n}/content   → raw bytes             (viewer)
//! GET    /api/v1/documents/{doc}/versions/{n}/thumbnail → image/png             (viewer; 404 if none)
//! POST   /api/v1/documents/{doc}/lock                   → Lock                  (editor)
//! DELETE /api/v1/documents/{doc}/lock                   → 204 (holder, or project admin breaks it)
//! GET    /api/v1/documents/{doc}/comments               → Vec<Comment>          (viewer)
//! POST   /api/v1/documents/{doc}/comments CreateCommentRequest → Comment        (commenter)
//! PATCH  /api/v1/comments/{comment}    UpdateCommentRequest → Comment           (body: author; resolve: author or editor+)
//! GET    /api/v1/documents/{doc}/shares                 → Vec<ShareLink>        (editor)
//! POST   /api/v1/documents/{doc}/shares CreateShareRequest → CreateShareResponse (editor)
//! DELETE /api/v1/shares/{share}                         → 204                   (editor)
//!
//! GET    /s/{token}                                     → HTML landing page     (no auth)
//! GET    /api/v1/shared/{token}                         → SharedDocument        (no auth)
//! GET    /api/v1/shared/{token}/content                 → raw bytes             (no auth, if allowed)
//! GET    /api/v1/shared/{token}/thumbnail               → image/png             (no auth; 404 if none)
//! ```
//!
//! # Saving a version
//!
//! 1. `POST /orgs/{org}/blobs/missing` with the hash of the document bytes.
//! 2. If it is missing, `PUT /orgs/{org}/blobs/{hash}` with the bytes. The server checks the
//!    hash and the size limit.
//! 3. `POST /documents/{doc}/versions` with `base_version` = the version the edit started
//!    from. If the head moved, the server answers 409 [`ErrorCode::VersionConflict`] and the
//!    client offers to save anyway (`force`) or to save as a new document. If another user
//!    holds the edit lock, the answer is 423 [`ErrorCode::Locked`].
//!
//! A version may carry a thumbnail: a PNG of at most [`MAX_THUMBNAIL_BYTES`], uploaded as a
//! blob like the document (steps 1–2) and named in [`CreateVersionRequest::thumbnail`]. The
//! server checks the PNG signature and size and refuses others with 400.

use serde::{Deserialize, Serialize};

/// Protocol version this crate describes ([`ServerInfo::protocol`]).
pub const PROTOCOL_VERSION: u32 = 1;
/// Path prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";
/// `product` value a conforming server reports in [`ServerInfo`].
pub const PRODUCT: &str = "rcad-server";
/// Prefix of session tokens returned by login.
pub const SESSION_TOKEN_PREFIX: &str = "rcs_";
/// Prefix of personal access tokens.
pub const ACCESS_TOKEN_PREFIX: &str = "rcp_";
/// Largest thumbnail PNG a server accepts.
pub const MAX_THUMBNAIL_BYTES: u64 = 1024 * 1024;

/// Lowercase hex BLAKE3 hash (64 characters) of a blob's bytes.
pub type BlobHash = String;
/// Unix time in seconds.
pub type Timestamp = i64;

/// True when `s` is a well-formed [`BlobHash`].
pub fn is_blob_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---------------------------------------------------------------------------------------------
// Errors

/// Stable error codes. Unknown codes from a newer server deserialize as `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 400: malformed request or invalid field.
    BadRequest,
    /// 401: missing, expired or revoked token, or wrong credentials.
    Unauthenticated,
    /// 403: authenticated but the role does not allow the action.
    Forbidden,
    /// 404: does not exist, or not visible to the caller.
    NotFound,
    /// 409: a name is already taken in that scope.
    AlreadyExists,
    /// 409: `base_version` is not the document's head.
    VersionConflict,
    /// 423: another user holds the document's edit lock.
    Locked,
    /// 413: blob larger than the server's limit.
    TooLarge,
    /// 507: the organization's storage quota would be exceeded.
    QuotaExceeded,
    /// 422: uploaded bytes do not match the hash in the path, or a referenced blob is missing.
    HashMismatch,
    /// 409: the folder still has content.
    NotEmpty,
    /// 410: the share link expired or was revoked.
    Gone,
    /// 429: too many requests (e.g. login attempts).
    RateLimited,
    /// 500.
    Internal,
    #[serde(other)]
    Other,
}

/// Error body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// Set with [`ErrorCode::VersionConflict`]: the current head version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_version: Option<u64>,
    /// Set with [`ErrorCode::Locked`]: who holds the lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<Lock>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

// ---------------------------------------------------------------------------------------------
// Server and auth

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// [`PRODUCT`].
    pub product: String,
    /// Server software version (informational).
    pub version: String,
    /// [`PROTOCOL_VERSION`] spoken by the server.
    pub protocol: u32,
    /// Human name of this deployment ("Home lab").
    pub name: String,
    /// Username/password login available.
    pub local_login: bool,
    /// Users may create organizations themselves.
    pub org_creation: bool,
    /// Largest blob accepted, in bytes.
    pub max_blob_bytes: u64,
    /// Single sign-on through an OpenID Connect provider, when configured.
    #[serde(default)]
    pub oidc: Option<OidcInfo>,
}

/// How a desktop client signs in through the server's OpenID Connect provider: authorization
/// code flow with PKCE (S256) and a loopback redirect (RFC 8252,
/// `http://127.0.0.1:<port>/callback`), then [`OidcExchangeRequest`] to the server, which
/// redeems the code with the provider, validates the ID token and returns a session token.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OidcInfo {
    /// Button label ("Sign in with Pocket ID").
    pub display_name: String,
    /// The provider's authorization endpoint (from its discovery document).
    pub authorization_endpoint: String,
    pub client_id: String,
    /// Space-separated scopes to request (includes `openid`).
    pub scopes: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OidcExchangeRequest {
    pub code: String,
    /// PKCE verifier whose S256 challenge was sent in the authorization request.
    pub code_verifier: String,
    /// The exact redirect URI used in the authorization request.
    pub redirect_uri: String,
    /// The `nonce` sent in the authorization request; the ID token must carry it.
    pub nonce: String,
    pub device_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Shown in the token list ("R-CAD on workstation").
    pub device_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: User,
    pub expires_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    /// Instance administrator (manages users and organizations on this server).
    pub server_admin: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub user: User,
    pub orgs: Vec<OrgSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    /// None = no expiry.
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTokenResponse {
    /// Shown once; the server keeps only a hash.
    pub token: String,
    pub info: TokenInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    pub kind: TokenKind,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Session,
    Personal,
}

// ---------------------------------------------------------------------------------------------
// Organizations

/// Role in an organization. Ordered: a higher role includes the lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    /// Sees only projects they are invited to; cannot create projects.
    Guest,
    /// Creates projects; sees projects they are a member of.
    Member,
    /// Manages members and every project (implicit project admin everywhere).
    Admin,
    /// Admin who can also delete the organization and manage other admins.
    Owner,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrgSummary {
    pub id: String,
    pub name: String,
    pub my_role: OrgRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrgMember {
    pub user: User,
    pub role: OrgRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetOrgMemberRequest {
    pub username: String,
    pub role: OrgRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrgUsage {
    /// Bytes of distinct blobs referenced by the organization.
    pub used_bytes: u64,
    /// None = unlimited.
    pub quota_bytes: Option<u64>,
    pub documents: u64,
    pub versions: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub at: Timestamp,
    pub actor: String,
    /// Machine-readable action, e.g. `version.create`, `member.set`, `share.create`.
    pub action: String,
    /// Human summary ("saved version 4 of Bracket").
    pub summary: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub document: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Blobs

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissingBlobsRequest {
    pub hashes: Vec<BlobHash>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissingBlobsResponse {
    /// The subset of the request the organization does not have yet.
    pub missing: Vec<BlobHash>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlobInfo {
    pub hash: BlobHash,
    pub size: u64,
}

// ---------------------------------------------------------------------------------------------
// Projects and folders

/// Role in a project. Ordered: a higher role includes the lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    /// Opens and downloads documents.
    Viewer,
    /// Viewer who can also comment.
    Commenter,
    /// Creates, edits, versions, moves and deletes documents and folders; creates share links.
    Editor,
    /// Editor who manages project members and settings and can break edit locks.
    Admin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub org: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Caller's effective role (org admins are project admins everywhere).
    pub my_role: ProjectRole,
    pub created_at: Timestamp,
    /// Increases on every change inside the project; poll it to detect changes.
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectMember {
    pub user: User,
    pub role: ProjectRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetProjectMemberRequest {
    pub username: String,
    pub role: ProjectRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub project: String,
    /// None = project root.
    pub parent: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateFolderRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// `Some(None)` moves to the project root.
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectTree {
    pub project: Project,
    pub folders: Vec<Folder>,
    /// Live documents (not in the trash).
    pub documents: Vec<DocumentSummary>,
    /// Documents in the trash, restorable by editors.
    #[serde(default)]
    pub trash: Vec<DocumentSummary>,
}

// ---------------------------------------------------------------------------------------------
// Documents and versions

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub id: String,
    pub project: String,
    pub folder: Option<String>,
    pub name: String,
    /// 0 when no version was saved yet.
    pub head_version: u64,
    pub updated_at: Timestamp,
    pub updated_by: String,
    #[serde(default)]
    pub lock: Option<Lock>,
    /// The head version has a thumbnail.
    #[serde(default)]
    pub has_thumbnail: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentDetail {
    pub document: DocumentSummary,
    /// Newest first.
    pub versions: Vec<Version>,
    pub my_role: ProjectRole,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateDocumentRequest {
    pub name: String,
    #[serde(default)]
    pub folder: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateDocumentRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// `Some(None)` moves to the project root.
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub folder: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub number: u64,
    pub blob: BlobHash,
    pub size: u64,
    pub author: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub description: String,
    /// Starred, named version kept by retention rules.
    #[serde(default)]
    pub milestone: Option<String>,
    /// Set when this version copies an earlier one ("promote").
    #[serde(default)]
    pub promoted_from: Option<u64>,
    /// Application that wrote it (`R-CAD 1.1.0 (abc1234)`), informational.
    #[serde(default)]
    pub generator: String,
    /// Blob of the version's PNG thumbnail, if it has one.
    #[serde(default)]
    pub thumbnail: Option<BlobHash>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateVersionRequest {
    /// Blob already uploaded to the document's organization.
    pub blob: BlobHash,
    /// Version the edit started from; 0 for a document with no versions. Ignored with `force`.
    pub base_version: u64,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub milestone: Option<String>,
    /// Save even if the head moved (the conflict is recorded in the activity log).
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub generator: String,
    /// Already-uploaded PNG blob (≤ [`MAX_THUMBNAIL_BYTES`]) shown for this version.
    #[serde(default)]
    pub thumbnail: Option<BlobHash>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateVersionRequest {
    #[serde(default)]
    pub description: Option<String>,
    /// `Some(None)` clears the milestone.
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub milestone: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromoteRequest {
    /// Must be the current head unless `force`.
    pub base_version: u64,
    #[serde(default)]
    pub force: bool,
}

/// Edit lock ("reservation"): while held, only the holder can save versions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub holder: String,
    pub since: Timestamp,
    /// Locks expire unless refreshed (POST again) so a crashed client cannot hold one forever.
    pub expires_at: Timestamp,
}

// ---------------------------------------------------------------------------------------------
// Comments

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub document: String,
    /// Version the comment was written against.
    pub version: u64,
    pub author: String,
    pub body: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub resolved: bool,
    /// Reply-to comment id.
    #[serde(default)]
    pub parent: Option<String>,
    /// Opaque client anchor (e.g. a serialized semantic face reference), JSON text.
    #[serde(default)]
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
    pub version: u64,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateCommentRequest {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub resolved: Option<bool>,
}

// ---------------------------------------------------------------------------------------------
// Share links

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateShareRequest {
    /// None = always the latest version.
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub expires_in_days: Option<u32>,
    /// Allow downloading the document file (otherwise metadata only).
    #[serde(default)]
    pub allow_download: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShareLink {
    pub id: String,
    pub document: String,
    pub version: Option<u64>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub allow_download: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateShareResponse {
    /// Absolute URL of the landing page (`<public url>/s/<token>`); shown once.
    pub url: String,
    pub token: String,
    pub link: ShareLink,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedDocument {
    pub name: String,
    pub version: SharedVersion,
    pub allow_download: bool,
}

/// A version as a share link shows it: no blob hash, so a view-only link never hands out
/// the address of the document's bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedVersion {
    pub number: u64,
    pub size: u64,
    pub author: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub has_thumbnail: bool,
}

// ---------------------------------------------------------------------------------------------

/// Serde helper for `Option<Option<T>>` fields: absent = `None`, `null` = `Some(None)`.
pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(
        v: &Option<Option<T>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => inner.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(OrgRole::Owner > OrgRole::Admin && OrgRole::Admin > OrgRole::Member);
        assert!(OrgRole::Member > OrgRole::Guest);
        assert!(ProjectRole::Admin > ProjectRole::Editor);
        assert!(ProjectRole::Editor > ProjectRole::Commenter);
        assert!(ProjectRole::Commenter > ProjectRole::Viewer);
    }

    #[test]
    fn unknown_error_codes_are_other() {
        let e: ApiError =
            serde_json::from_str(r#"{"code":"from_the_future","message":"x"}"#).unwrap();
        assert_eq!(e.code, ErrorCode::Other);
    }

    #[test]
    fn double_option_distinguishes_absent_and_null() {
        let absent: UpdateDocumentRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.folder, None);
        let null: UpdateDocumentRequest = serde_json::from_str(r#"{"folder":null}"#).unwrap();
        assert_eq!(null.folder, Some(None));
        let set: UpdateDocumentRequest = serde_json::from_str(r#"{"folder":"f1"}"#).unwrap();
        assert_eq!(set.folder, Some(Some("f1".into())));
        // Absent fields round-trip as absent only when skipped; null is accepted either way.
        let req = UpdateVersionRequest {
            milestone: Some(None),
            ..Default::default()
        };
        let back: UpdateVersionRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(back.milestone, Some(None));
    }

    #[test]
    fn blob_hash_shape() {
        assert!(is_blob_hash(&"a".repeat(64)));
        assert!(!is_blob_hash(&"A".repeat(64)));
        assert!(!is_blob_hash(&"a".repeat(63)));
        assert!(!is_blob_hash(&"g".repeat(64)));
    }
}
