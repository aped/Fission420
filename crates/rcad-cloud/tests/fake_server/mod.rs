//! In-memory fake of the sync server protocol for hermetic tests (127.0.0.1 only).
//!
//! Implements the routes the client library uses with the protocol's status codes and error
//! bodies. Knobs on [`State`] simulate hostile or unusual servers.
#![allow(dead_code)]

use rcad_cloud::proto::{self, ApiError, ErrorCode};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const USER: &str = "ada";
pub const PASSWORD: &str = "correct horse";
pub const ORG: &str = "o1";
pub const PROJECT: &str = "p1";

#[derive(Clone)]
pub struct Doc {
    pub summary: proto::DocumentSummary,
    pub versions: Vec<proto::Version>, // oldest first
    pub lock: Option<proto::Lock>,
}

pub struct State {
    pub info: proto::ServerInfo,
    pub tokens: HashMap<String, String>,
    pub docs: Vec<Doc>,
    pub blobs: HashMap<String, Vec<u8>>,
    pub comments: Vec<proto::Comment>,
    pub folders: Vec<proto::Folder>,
    pub projects: Vec<proto::Project>,
    pub log: Vec<String>,
    pub next_id: u64,
    /// Serve content with its last byte changed (hash mismatch, same size).
    pub tamper_content: bool,
    /// Serve content with this many extra bytes beyond the version's declared size.
    pub inflate_content: usize,
    /// Answer this many version saves by deleting the referenced blob and failing with 422
    /// `hash_mismatch` (a garbage collection racing the save).
    pub drop_blob_on_save: u32,
    pub logins: Vec<proto::LoginRequest>,
    /// Documents in the trash.
    pub trash: Vec<Doc>,
    pub org_members: Vec<proto::OrgMember>,
    /// Explicit project members (project id, member).
    pub project_members: Vec<(String, proto::ProjectMember)>,
    pub usage: proto::OrgUsage,
    /// Answer every version save with this error (e.g. a full quota).
    pub fail_saves: Option<(u16, ErrorCode)>,
}

fn user(name: &str) -> proto::User {
    proto::User {
        id: format!("u-{name}"),
        username: name.into(),
        display_name: name.into(),
        server_admin: false,
    }
}

impl State {
    fn new() -> State {
        State {
            info: proto::ServerInfo {
                product: proto::PRODUCT.into(),
                version: "test".into(),
                protocol: proto::PROTOCOL_VERSION,
                name: "Test lab".into(),
                local_login: true,
                org_creation: false,
                max_blob_bytes: 1 << 30,
            },
            tokens: HashMap::new(),
            docs: Vec::new(),
            blobs: HashMap::new(),
            comments: Vec::new(),
            folders: Vec::new(),
            projects: vec![proto::Project {
                id: PROJECT.into(),
                org: ORG.into(),
                name: "Widgets".into(),
                description: String::new(),
                my_role: proto::ProjectRole::Admin,
                created_at: 1,
                revision: 1,
            }],
            log: Vec::new(),
            next_id: 1,
            tamper_content: false,
            inflate_content: 0,
            drop_blob_on_save: 0,
            logins: Vec::new(),
            trash: Vec::new(),
            org_members: vec![
                proto::OrgMember {
                    user: user(USER),
                    role: proto::OrgRole::Owner,
                },
                proto::OrgMember {
                    user: user("bob"),
                    role: proto::OrgRole::Member,
                },
            ],
            project_members: Vec::new(),
            usage: proto::OrgUsage {
                used_bytes: 1_000,
                quota_bytes: Some(10_000),
                documents: 0,
                versions: 0,
            },
            fail_saves: None,
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    pub fn doc_mut(&mut self, id: &str) -> Option<&mut Doc> {
        self.docs.iter_mut().find(|d| d.summary.id == id)
    }

    /// Save a version as another user (moves the head).
    pub fn save_as(&mut self, doc: &str, author: &str, bytes: &[u8]) -> u64 {
        let hash = rcad_cloud::blob_hash(bytes);
        self.blobs.insert(hash.clone(), bytes.to_vec());
        let d = self.doc_mut(doc).expect("doc");
        let n = d.versions.len() as u64 + 1;
        d.versions.push(proto::Version {
            number: n,
            blob: hash,
            size: bytes.len() as u64,
            author: author.into(),
            created_at: 100 + n as i64,
            description: String::new(),
            milestone: None,
            promoted_from: None,
            generator: String::new(),
        });
        d.summary.head_version = n;
        n
    }

    fn detail(&self, d: &Doc) -> proto::DocumentDetail {
        let mut versions = d.versions.clone();
        versions.reverse();
        proto::DocumentDetail {
            document: d.summary.clone(),
            versions,
            my_role: proto::ProjectRole::Editor,
        }
    }
}

pub struct FakeServer {
    pub url: String,
    pub port: u16,
    pub state: Arc<Mutex<State>>,
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start() -> FakeServer {
        Self::start_on(0, Arc::new(Mutex::new(State::new())))
    }

    /// Serve `state` on `port` (0 = any free port).
    pub fn start_on(port: u16, state: Arc<Mutex<State>>) -> FakeServer {
        let server = Arc::new(tiny_http::Server::http(("127.0.0.1", port)).expect("bind"));
        let port = server.server_addr().to_ip().expect("ip").port();
        let s2 = server.clone();
        let st = state.clone();
        let thread = std::thread::spawn(move || {
            for req in s2.incoming_requests() {
                handle(&st, req);
            }
        });
        FakeServer {
            url: format!("http://127.0.0.1:{port}"),
            port,
            state,
            server,
            thread: Some(thread),
        }
    }

    /// Stop listening (the port is free again afterwards).
    pub fn stop(mut self) -> Arc<Mutex<State>> {
        self.shutdown();
        self.state.clone()
    }

    fn shutdown(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    pub fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A port with nothing listening on it.
pub fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

type Reply = (u16, Vec<u8>, &'static str);

fn json<T: Serialize>(status: u16, v: &T) -> Reply {
    (status, serde_json::to_vec(v).unwrap(), "application/json")
}

fn err(status: u16, code: ErrorCode, msg: &str) -> Reply {
    json(
        status,
        &ApiError {
            code,
            message: msg.into(),
            head_version: None,
            lock: None,
        },
    )
}

fn handle(state: &Arc<Mutex<State>>, mut req: tiny_http::Request) {
    let method = req.method().as_str().to_string();
    let url = req.url().to_string();
    let auth = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string());
    let mut body = Vec::new();
    let _ = req.as_reader().read_to_end(&mut body);
    let (status, bytes, ctype) = {
        let mut st = state.lock().unwrap();
        st.log.push(format!("{method} {url}"));
        route(&mut st, &method, &url, auth.as_deref(), &body)
    };
    let resp = tiny_http::Response::from_data(bytes)
        .with_status_code(status)
        .with_header(tiny_http::Header::from_bytes("Content-Type", ctype).unwrap());
    let _ = req.respond(resp);
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(body).map_err(|e| err(400, ErrorCode::BadRequest, &e.to_string()))
}

fn route(st: &mut State, method: &str, url: &str, auth: Option<&str>, body: &[u8]) -> Reply {
    let path = url.split('?').next().unwrap_or("");
    let Some(rest) = path.strip_prefix(proto::API_PREFIX) else {
        return err(404, ErrorCode::NotFound, "no route");
    };
    let segs: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match (method, segs.as_slice()) {
        ("GET", ["info"]) => return json(200, &st.info),
        ("POST", ["auth", "login"]) => {
            let req: proto::LoginRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            st.logins.push(req.clone());
            if req.username != USER || req.password != PASSWORD {
                return err(
                    401,
                    ErrorCode::Unauthenticated,
                    "wrong username or password",
                );
            }
            let token = format!("{}{}", proto::SESSION_TOKEN_PREFIX, st.id("tok"));
            st.tokens.insert(token.clone(), USER.into());
            return json(
                200,
                &proto::LoginResponse {
                    token,
                    user: user(USER),
                    expires_at: 9_999_999_999,
                },
            );
        }
        _ => {}
    }
    let who = match auth
        .and_then(|a| a.strip_prefix("Bearer "))
        .and_then(|t| st.tokens.get(t))
    {
        Some(u) => u.clone(),
        None => return err(401, ErrorCode::Unauthenticated, "sign in"),
    };
    match (method, segs.as_slice()) {
        ("POST", ["auth", "logout"]) => {
            let t = auth.unwrap_or("").trim_start_matches("Bearer ").to_string();
            st.tokens.remove(&t);
            (204, Vec::new(), "application/json")
        }
        ("GET", ["me"]) => json(
            200,
            &proto::Me {
                user: user(&who),
                orgs: vec![proto::OrgSummary {
                    id: ORG.into(),
                    name: "Acme".into(),
                    my_role: proto::OrgRole::Owner,
                }],
            },
        ),
        ("GET", ["orgs", _, "members"]) => json(200, &st.org_members),
        ("PUT", ["orgs", _, "members"]) => {
            let req: proto::SetOrgMemberRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let m = proto::OrgMember {
                user: user(&req.username),
                role: req.role,
            };
            match st
                .org_members
                .iter_mut()
                .find(|x| x.user.username.eq_ignore_ascii_case(&req.username))
            {
                Some(x) => x.role = req.role,
                None => st.org_members.push(m.clone()),
            }
            json(200, &m)
        }
        ("DELETE", ["orgs", _, "members", name]) => {
            let before = st.org_members.len();
            st.org_members.retain(|x| x.user.username != *name);
            if st.org_members.len() == before {
                return err(404, ErrorCode::NotFound, "no such member");
            }
            st.project_members.retain(|(_, m)| m.user.username != *name);
            (204, Vec::new(), "application/json")
        }
        ("GET", ["orgs", _, "usage"]) => json(200, &st.usage),
        ("GET", ["projects", p, "members"]) => {
            let list: Vec<_> = st
                .project_members
                .iter()
                .filter(|(q, _)| q == p)
                .map(|(_, m)| m.clone())
                .collect();
            json(200, &list)
        }
        ("PUT", ["projects", p, "members"]) => {
            let req: proto::SetProjectMemberRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if !st
                .org_members
                .iter()
                .any(|x| x.user.username.eq_ignore_ascii_case(&req.username))
            {
                return err(404, ErrorCode::NotFound, "no such user in the organization");
            }
            let m = proto::ProjectMember {
                user: user(&req.username),
                role: req.role,
            };
            match st
                .project_members
                .iter_mut()
                .find(|(q, x)| q == p && x.user.username == req.username)
            {
                Some((_, x)) => x.role = req.role,
                None => st.project_members.push((p.to_string(), m.clone())),
            }
            json(200, &m)
        }
        ("DELETE", ["projects", p, "members", name]) => {
            let before = st.project_members.len();
            st.project_members
                .retain(|(q, m)| !(q == p && m.user.username == *name));
            if st.project_members.len() == before {
                return err(404, ErrorCode::NotFound, "not a member");
            }
            (204, Vec::new(), "application/json")
        }
        ("PATCH", ["folders", f]) => {
            let req: proto::UpdateFolderRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let Some(folder) = st.folders.iter_mut().find(|x| x.id == *f) else {
                return err(404, ErrorCode::NotFound, "no folder");
            };
            if let Some(n) = req.name {
                folder.name = n;
            }
            if let Some(p) = req.parent {
                folder.parent = p;
            }
            json(200, &folder.clone())
        }
        ("DELETE", ["folders", f]) => {
            let used = st.folders.iter().any(|x| x.parent.as_deref() == Some(*f))
                || st
                    .docs
                    .iter()
                    .chain(st.trash.iter())
                    .any(|d| d.summary.folder.as_deref() == Some(*f));
            if used {
                return err(409, ErrorCode::NotEmpty, "the folder is not empty");
            }
            let before = st.folders.len();
            st.folders.retain(|x| x.id != *f);
            if st.folders.len() == before {
                return err(404, ErrorCode::NotFound, "no folder");
            }
            (204, Vec::new(), "application/json")
        }
        ("PATCH", ["documents", d]) => {
            let req: proto::UpdateDocumentRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if let Some(name) = &req.name {
                let project = st.doc_mut(d).map(|x| x.summary.project.clone());
                if st.docs.iter().any(|x| {
                    x.summary.id != *d
                        && Some(&x.summary.project) == project.as_ref()
                        && x.summary.name.eq_ignore_ascii_case(name)
                }) {
                    return err(409, ErrorCode::AlreadyExists, "name taken");
                }
            }
            let Some(doc) = st.doc_mut(d) else {
                return err(404, ErrorCode::NotFound, "no document");
            };
            if let Some(n) = req.name {
                doc.summary.name = n;
            }
            if let Some(f) = req.folder {
                doc.summary.folder = f;
            }
            json(200, &doc.summary.clone())
        }
        ("DELETE", ["documents", d]) => {
            let Some(i) = st.docs.iter().position(|x| x.summary.id == *d) else {
                return err(404, ErrorCode::NotFound, "no document");
            };
            let doc = st.docs.remove(i);
            st.trash.push(doc);
            (204, Vec::new(), "application/json")
        }
        ("POST", ["documents", d, "restore"]) => {
            let Some(i) = st.trash.iter().position(|x| x.summary.id == *d) else {
                return err(404, ErrorCode::NotFound, "not in the trash");
            };
            let doc = st.trash.remove(i);
            let summary = doc.summary.clone();
            st.docs.push(doc);
            json(200, &summary)
        }
        ("GET", ["orgs", _, "projects"]) => json(200, &st.projects),
        ("POST", ["orgs", org, "projects"]) => {
            let req: proto::CreateProjectRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let p = proto::Project {
                id: st.id("p"),
                org: org.to_string(),
                name: req.name,
                description: req.description,
                my_role: proto::ProjectRole::Admin,
                created_at: 2,
                revision: 1,
            };
            st.projects.push(p.clone());
            json(200, &p)
        }
        ("POST", ["orgs", _, "blobs", "missing"]) => {
            let req: proto::MissingBlobsRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let missing = req
                .hashes
                .into_iter()
                .filter(|h| !st.blobs.contains_key(h))
                .collect();
            json(200, &proto::MissingBlobsResponse { missing })
        }
        ("PUT", ["orgs", _, "blobs", hash]) => {
            if rcad_cloud::blob_hash(body) != *hash {
                return err(422, ErrorCode::HashMismatch, "hash mismatch");
            }
            st.blobs.insert(hash.to_string(), body.to_vec());
            json(
                200,
                &proto::BlobInfo {
                    hash: hash.to_string(),
                    size: body.len() as u64,
                },
            )
        }
        ("GET", ["projects", p]) => match st.projects.iter().find(|x| x.id == *p) {
            Some(p) => json(200, p),
            None => err(404, ErrorCode::NotFound, "no project"),
        },
        ("GET", ["projects", p, "tree"]) => {
            let Some(project) = st.projects.iter().find(|x| x.id == *p).cloned() else {
                return err(404, ErrorCode::NotFound, "no project");
            };
            json(
                200,
                &proto::ProjectTree {
                    project,
                    folders: st
                        .folders
                        .iter()
                        .filter(|f| f.project == *p)
                        .cloned()
                        .collect(),
                    documents: st
                        .docs
                        .iter()
                        .filter(|d| d.summary.project == *p)
                        .map(|d| d.summary.clone())
                        .collect(),
                    trash: st
                        .trash
                        .iter()
                        .filter(|d| d.summary.project == *p)
                        .map(|d| d.summary.clone())
                        .collect(),
                },
            )
        }
        ("POST", ["projects", p, "folders"]) => {
            let req: proto::CreateFolderRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let f = proto::Folder {
                id: st.id("f"),
                project: p.to_string(),
                parent: req.parent,
                name: req.name,
            };
            st.folders.push(f.clone());
            json(200, &f)
        }
        ("POST", ["projects", p, "documents"]) => {
            let req: proto::CreateDocumentRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let id = st.id("d");
            let summary = proto::DocumentSummary {
                id,
                project: p.to_string(),
                folder: req.folder,
                name: req.name,
                head_version: 0,
                updated_at: 1,
                updated_by: who.clone(),
                lock: None,
            };
            st.docs.push(Doc {
                summary: summary.clone(),
                versions: Vec::new(),
                lock: None,
            });
            json(200, &summary)
        }
        ("GET", ["documents", d]) => match st.docs.iter().find(|x| x.summary.id == *d) {
            Some(doc) => json(200, &st.detail(doc)),
            None => err(404, ErrorCode::NotFound, "no document"),
        },
        ("POST", ["documents", d, "versions"]) => {
            let req: proto::CreateVersionRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if let Some((status, code)) = st.fail_saves {
                return err(status, code, "save refused");
            }
            if st.drop_blob_on_save > 0 {
                st.drop_blob_on_save -= 1;
                st.blobs.remove(&req.blob);
            }
            if !st.blobs.contains_key(&req.blob) {
                return err(422, ErrorCode::HashMismatch, "blob missing");
            }
            let blob_len = st.blobs[&req.blob].len() as u64;
            let Some(doc) = st.doc_mut(d) else {
                return err(404, ErrorCode::NotFound, "no document");
            };
            if let Some(lock) = doc.lock.clone().filter(|l| l.holder != who) {
                return json(
                    423,
                    &ApiError {
                        code: ErrorCode::Locked,
                        message: format!("locked by {}", lock.holder),
                        head_version: None,
                        lock: Some(lock),
                    },
                );
            }
            let head = doc.summary.head_version;
            if !req.force && req.base_version != head {
                return json(
                    409,
                    &ApiError {
                        code: ErrorCode::VersionConflict,
                        message: "head moved".into(),
                        head_version: Some(head),
                        lock: None,
                    },
                );
            }
            let v = proto::Version {
                number: head + 1,
                blob: req.blob,
                size: blob_len,
                author: who.clone(),
                created_at: 200 + head as i64,
                description: req.description,
                milestone: req.milestone,
                promoted_from: None,
                generator: req.generator,
            };
            doc.versions.push(v.clone());
            doc.summary.head_version = v.number;
            json(200, &v)
        }
        ("PATCH", ["documents", d, "versions", n]) => {
            let req: proto::UpdateVersionRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let n: u64 = n.parse().unwrap_or(0);
            let Some(v) = st
                .doc_mut(d)
                .and_then(|doc| doc.versions.iter_mut().find(|v| v.number == n))
            else {
                return err(404, ErrorCode::NotFound, "no version");
            };
            if let Some(m) = req.milestone {
                v.milestone = m;
            }
            json(200, &v.clone())
        }
        ("POST", ["documents", d, "versions", n, "promote"]) => {
            let req: proto::PromoteRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let n: u64 = n.parse().unwrap_or(0);
            let Some(doc) = st.doc_mut(d) else {
                return err(404, ErrorCode::NotFound, "no document");
            };
            if !req.force && req.base_version != doc.summary.head_version {
                return err(409, ErrorCode::VersionConflict, "head moved");
            }
            let Some(src) = doc.versions.iter().find(|v| v.number == n).cloned() else {
                return err(404, ErrorCode::NotFound, "no version");
            };
            let v = proto::Version {
                number: doc.summary.head_version + 1,
                promoted_from: Some(n),
                author: who.clone(),
                ..src
            };
            doc.versions.push(v.clone());
            doc.summary.head_version = v.number;
            json(200, &v)
        }
        ("GET", ["documents", d, "versions", n, "content"]) => {
            let n: u64 = n.parse().unwrap_or(0);
            let tamper = st.tamper_content;
            let inflate = st.inflate_content;
            let Some(v) = st
                .docs
                .iter()
                .find(|x| x.summary.id == *d)
                .and_then(|doc| doc.versions.iter().find(|v| v.number == n))
            else {
                return err(404, ErrorCode::NotFound, "no version");
            };
            let mut bytes = st.blobs.get(&v.blob).cloned().unwrap_or_default();
            if tamper && let Some(last) = bytes.last_mut() {
                *last ^= 0x20;
            }
            bytes.extend(std::iter::repeat_n(b'x', inflate));
            (200, bytes, "application/octet-stream")
        }
        ("POST", ["documents", d, "lock"]) => {
            let Some(doc) = st.doc_mut(d) else {
                return err(404, ErrorCode::NotFound, "no document");
            };
            if let Some(lock) = doc.lock.clone().filter(|l| l.holder != who) {
                return json(
                    423,
                    &ApiError {
                        code: ErrorCode::Locked,
                        message: "locked".into(),
                        head_version: None,
                        lock: Some(lock),
                    },
                );
            }
            let lock = proto::Lock {
                holder: who.clone(),
                since: 10,
                expires_at: 10_000,
            };
            doc.lock = Some(lock.clone());
            doc.summary.lock = Some(lock.clone());
            json(200, &lock)
        }
        ("DELETE", ["documents", d, "lock"]) => {
            if let Some(doc) = st.doc_mut(d) {
                doc.lock = None;
                doc.summary.lock = None;
            }
            (204, Vec::new(), "application/json")
        }
        ("GET", ["documents", d, "comments"]) => {
            let c: Vec<_> = st
                .comments
                .iter()
                .filter(|c| c.document == *d)
                .cloned()
                .collect();
            json(200, &c)
        }
        ("POST", ["documents", d, "comments"]) => {
            let req: proto::CreateCommentRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let c = proto::Comment {
                id: st.id("c"),
                document: d.to_string(),
                version: req.version,
                author: who.clone(),
                body: req.body,
                created_at: 5,
                resolved: false,
                parent: req.parent,
                anchor: req.anchor,
            };
            st.comments.push(c.clone());
            json(200, &c)
        }
        ("PATCH", ["comments", c]) => {
            let req: proto::UpdateCommentRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let Some(cm) = st.comments.iter_mut().find(|x| x.id == *c) else {
                return err(404, ErrorCode::NotFound, "no comment");
            };
            if let Some(r) = req.resolved {
                cm.resolved = r;
            }
            json(200, &cm.clone())
        }
        ("POST", ["documents", d, "shares"]) => {
            let req: proto::CreateShareRequest = match parse(body) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let token = st.id("share");
            json(
                200,
                &proto::CreateShareResponse {
                    url: format!("http://example.invalid/s/{token}"),
                    token: token.clone(),
                    link: proto::ShareLink {
                        id: token,
                        document: d.to_string(),
                        version: req.version,
                        created_by: who.clone(),
                        created_at: 7,
                        expires_at: req.expires_in_days.map(|d| 7 + d as i64 * 86_400),
                        allow_download: req.allow_download,
                    },
                },
            )
        }
        _ => err(
            404,
            ErrorCode::NotFound,
            &format!("no route {method} {path}"),
        ),
    }
}
