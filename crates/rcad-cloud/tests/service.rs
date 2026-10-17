//! Service behaviour against an in-memory fake server on 127.0.0.1 (no external network).
mod fake_server;

use fake_server::{FakeServer, PASSWORD, PROJECT, USER};
use rcad_cloud::cache::ServerCache;
use rcad_cloud::config::CloudConfig;
use rcad_cloud::proto::{self, ErrorCode};
use rcad_cloud::service::{CloudService, Event, Request, Worker};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Harness {
    dir: tempfile::TempDir,
    worker: Worker,
}

impl Harness {
    fn new() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::new(
            dir.path().join("config/cloud.json"),
            dir.path().join("cache"),
        );
        Harness { dir, worker }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.path().join("config/cloud.json")
    }

    fn cache_root(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn run(&mut self, req: Request) -> Vec<Event> {
        let mut out = Vec::new();
        self.worker.handle(req, &mut |e| out.push(e));
        out
    }

    fn sign_in(&mut self, url: &str) {
        let ev = self.run(Request::SignIn {
            server_url: url.into(),
            username: USER.into(),
            password: PASSWORD.into(),
        });
        assert!(matches!(ev.as_slice(), [Event::SignedIn { .. }]), "{ev:?}");
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn cache(&self) -> ServerCache {
        self.worker.cache().unwrap().unwrap()
    }

    /// Upload `bytes` as a new document; returns (file, document id).
    fn upload(&mut self, name: &str, bytes: &[u8]) -> (PathBuf, String) {
        let path = self.file(&format!("{name}.rcad.json"), bytes);
        let ev = self.run(Request::UploadNew {
            local_path: path.clone(),
            project: PROJECT.into(),
            folder: None,
            name: name.into(),
        });
        match ev.as_slice() {
            [Event::VersionSaved { link, version }] => {
                assert_eq!(version.number, 1);
                assert_eq!(link.base_version, 1);
                (path, link.document.clone())
            }
            other => panic!("{other:?}"),
        }
    }

    fn save(&mut self, path: &Path, force: bool) -> Vec<Event> {
        self.run(Request::SaveVersion {
            local_path: path.to_path_buf(),
            description: "edit".into(),
            milestone: None,
            force,
        })
    }
}

/// A document created on the server by someone else, with one version.
fn server_doc(server: &FakeServer, name: &str, bytes: &[u8]) -> String {
    let mut st = server.state.lock().unwrap();
    let id = format!("d-{}", st.docs.len() + 100);
    st.docs.push(fake_server::Doc {
        summary: proto::DocumentSummary {
            id: id.clone(),
            project: PROJECT.into(),
            folder: None,
            name: name.into(),
            head_version: 0,
            updated_at: 1,
            updated_by: "bob".into(),
            lock: None,
        },
        versions: Vec::new(),
        lock: None,
    });
    st.save_as(&id, "bob", bytes);
    id
}

#[test]
fn sign_in_stores_the_account_and_wrong_password_does_not() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    let ev = h.run(Request::SignIn {
        server_url: server.url.clone(),
        username: USER.into(),
        password: "wrong".into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "sign_in",
                code: Some(ErrorCode::Unauthenticated),
                ..
            }]
        ),
        "{ev:?}"
    );
    assert!(
        CloudConfig::load(&h.config_path())
            .unwrap()
            .accounts
            .is_empty()
    );

    let ev = h.run(Request::SignIn {
        server_url: format!("{}/", server.url),
        username: USER.into(),
        password: PASSWORD.into(),
    });
    match ev.as_slice() {
        [
            Event::SignedIn {
                server_url,
                server_name,
                me,
            },
        ] => {
            assert_eq!(server_url, &server.url);
            assert_eq!(server_name, "Test lab");
            assert_eq!(me.user.username, USER);
            assert_eq!(me.orgs.len(), 1);
        }
        other => panic!("{other:?}"),
    }
    let cfg = CloudConfig::load(&h.config_path()).unwrap();
    let acct = cfg.active_account().unwrap();
    assert_eq!(acct.server_url, server.url);
    assert!(acct.token.starts_with(proto::SESSION_TOKEN_PREFIX));
    let logins = server.state.lock().unwrap().logins.clone();
    assert!(logins.last().unwrap().device_name.starts_with("R-CAD on "));

    // Refresh reloads `me` with the stored token.
    let ev = h.run(Request::Refresh);
    assert!(matches!(ev.as_slice(), [Event::SignedIn { .. }]), "{ev:?}");

    // Sign out revokes the token and forgets the account.
    let ev = h.run(Request::SignOut);
    assert_eq!(ev, vec![Event::SignedOut]);
    assert!(
        CloudConfig::load(&h.config_path())
            .unwrap()
            .accounts
            .is_empty()
    );
    assert!(server.state.lock().unwrap().tokens.is_empty());
    let ev = h.run(Request::Refresh);
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "refresh",
                code: None,
                ..
            }]
        ),
        "{ev:?}"
    );
}

#[test]
fn protocol_or_product_mismatch_is_refused_before_login() {
    let server = FakeServer::start();
    server.state.lock().unwrap().info.protocol = proto::PROTOCOL_VERSION + 1;
    let mut h = Harness::new();
    let ev = h.run(Request::SignIn {
        server_url: server.url.clone(),
        username: USER.into(),
        password: PASSWORD.into(),
    });
    match ev.as_slice() {
        [
            Event::Error {
                request: "sign_in",
                message,
                ..
            },
        ] => assert!(message.contains("protocol"), "{message}"),
        other => panic!("{other:?}"),
    }
    {
        let mut st = server.state.lock().unwrap();
        st.info.protocol = proto::PROTOCOL_VERSION;
        st.info.product = "something-else".into();
    }
    let ev = h.run(Request::SignIn {
        server_url: server.url.clone(),
        username: USER.into(),
        password: PASSWORD.into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "sign_in",
                ..
            }]
        ),
        "{ev:?}"
    );
    assert!(
        server.state.lock().unwrap().logins.is_empty(),
        "password never sent"
    );
    assert!(
        CloudConfig::load(&h.config_path())
            .unwrap()
            .accounts
            .is_empty()
    );
}

#[test]
fn sign_in_to_an_unreachable_server_is_an_error() {
    let mut h = Harness::new();
    let ev = h.run(Request::SignIn {
        server_url: format!("http://127.0.0.1:{}", fake_server::dead_port()),
        username: USER.into(),
        password: PASSWORD.into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "sign_in",
                ..
            }]
        ),
        "{ev:?}"
    );
}

#[test]
fn upload_new_then_save_versions_update_the_link() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Bracket", b"{\"v\":1}");
    let link = h.cache().link_for_path(&path).unwrap().unwrap();
    assert_eq!(link.document, doc);
    assert_eq!(link.org, fake_server::ORG);
    assert_eq!(
        link.base_blob.as_deref(),
        Some(rcad_cloud::blob_hash(b"{\"v\":1}").as_str())
    );
    // The local file itself is linked, not copied.
    assert_eq!(link.local_path, std::fs::canonicalize(&path).unwrap());

    std::fs::write(&path, b"{\"v\":2}").unwrap();
    let ev = h.run(Request::SaveVersion {
        local_path: path.clone(),
        description: "thicker".into(),
        milestone: Some("Prototype A".into()),
        force: false,
    });
    match ev.as_slice() {
        [Event::VersionSaved { link, version }] => {
            assert_eq!(version.number, 2);
            assert_eq!(version.description, "thicker");
            assert_eq!(version.milestone.as_deref(), Some("Prototype A"));
            assert!(version.generator.starts_with("R-CAD "));
            assert_eq!(link.base_version, 2);
        }
        other => panic!("{other:?}"),
    }
    let st = server.state.lock().unwrap();
    assert_eq!(st.blobs[&rcad_cloud::blob_hash(b"{\"v\":2}")], b"{\"v\":2}");
    drop(st);

    // Saving unchanged bytes again skips the upload (blob already on the server).
    let before = server.log().iter().filter(|l| l.starts_with("PUT")).count();
    let ev = h.save(&path, false);
    assert!(
        matches!(ev.as_slice(), [Event::VersionSaved { .. }]),
        "{ev:?}"
    );
    let after = server.log().iter().filter(|l| l.starts_with("PUT")).count();
    assert_eq!(before, after);

    // An unlinked file is refused locally.
    let other = h.file("loose.rcad.json", b"{}");
    let ev = h.save(&other, false);
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "save_version",
                code: None,
                ..
            }]
        ),
        "{ev:?}"
    );
}

#[test]
fn conflict_is_reported_and_force_saves() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Plate", b"one");
    server
        .state
        .lock()
        .unwrap()
        .save_as(&doc, "bob", b"bob's two");
    std::fs::write(&path, b"my two").unwrap();
    let ev = h.save(&path, false);
    match ev.as_slice() {
        [
            Event::Conflict {
                local_path,
                document,
                base_version,
                head_version,
            },
        ] => {
            assert_eq!(local_path, &std::fs::canonicalize(&path).unwrap());
            assert_eq!(document, &doc);
            assert_eq!((*base_version, *head_version), (1, 2));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        h.cache()
            .link_for_path(&path)
            .unwrap()
            .unwrap()
            .base_version,
        1
    );
    let ev = h.save(&path, true);
    match ev.as_slice() {
        [Event::VersionSaved { link, version }] => {
            assert_eq!(version.number, 3);
            assert_eq!(link.base_version, 3);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn locked_document_reports_the_holder() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Hinge", b"one");
    server.state.lock().unwrap().doc_mut(&doc).unwrap().lock = Some(proto::Lock {
        holder: "bob".into(),
        since: 1,
        expires_at: 99,
    });
    std::fs::write(&path, b"two").unwrap();
    let ev = h.save(&path, false);
    match ev.as_slice() {
        [Event::Locked { document, lock }] => {
            assert_eq!(document, &doc);
            assert_eq!(lock.holder, "bob");
        }
        other => panic!("{other:?}"),
    }
    let ev = h.run(Request::Lock {
        document: doc.clone(),
    });
    assert!(matches!(ev.as_slice(), [Event::Locked { .. }]), "{ev:?}");
    server.state.lock().unwrap().doc_mut(&doc).unwrap().lock = None;
    let ev = h.run(Request::Lock {
        document: doc.clone(),
    });
    assert!(
        matches!(&ev[..], [Event::LockAcquired { lock, .. }] if lock.holder == USER),
        "{ev:?}"
    );
    let ev = h.run(Request::Unlock {
        document: doc.clone(),
    });
    assert_eq!(ev, vec![Event::LockReleased { document: doc }]);
}

#[test]
fn offline_saves_queue_and_sync_in_order() {
    let server = FakeServer::start();
    let port = server.port;
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Shaft", b"v1");
    let state = server.stop();

    std::fs::write(&path, b"v2").unwrap();
    assert_eq!(h.save(&path, false), vec![Event::Offline { pending: 1 }]);
    std::fs::write(&path, b"v3").unwrap();
    assert_eq!(h.save(&path, false), vec![Event::Offline { pending: 2 }]);
    // Later edits do not change what was queued.
    std::fs::write(&path, b"v4 unsaved").unwrap();
    // Still offline: sync keeps the queue.
    assert_eq!(
        h.run(Request::SyncPending),
        vec![Event::Offline { pending: 2 }]
    );
    // Other requests report Offline too.
    assert_eq!(
        h.run(Request::LoadDetail {
            document: doc.clone()
        }),
        vec![Event::Offline { pending: 2 }]
    );

    let server = FakeServer::start_on(port, state);
    let ev = h.run(Request::SyncPending);
    match ev.as_slice() {
        [
            Event::VersionSaved { version: a, .. },
            Event::VersionSaved { version: b, link },
            Event::Synced { pending: 0 },
        ] => {
            assert_eq!((a.number, b.number), (2, 3));
            assert_eq!(a.blob, rcad_cloud::blob_hash(b"v2"));
            assert_eq!(b.blob, rcad_cloud::blob_hash(b"v3"));
            assert_eq!(link.base_version, 3);
            assert_eq!(
                link.base_blob.as_deref(),
                Some(rcad_cloud::blob_hash(b"v3").as_str())
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"v4 unsaved");
    assert!(h.cache().pending().unwrap().is_empty());
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .doc_mut(&doc)
            .unwrap()
            .versions
            .len(),
        3
    );
}

#[test]
fn sync_stops_at_a_conflict_and_keeps_it_queued() {
    let server = FakeServer::start();
    let port = server.port;
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Gear", b"v1");
    let state = server.stop();
    std::fs::write(&path, b"mine").unwrap();
    assert_eq!(h.save(&path, false), vec![Event::Offline { pending: 1 }]);
    state.lock().unwrap().save_as(&doc, "bob", b"bob");
    let _server = FakeServer::start_on(port, state);
    let ev = h.run(Request::SyncPending);
    assert!(
        matches!(
            ev.as_slice(),
            [
                Event::Conflict {
                    base_version: 1,
                    head_version: 2,
                    ..
                },
                Event::Synced { pending: 1 }
            ]
        ),
        "{ev:?}"
    );
    // Resolving with force replays the queued save forcibly, then the current file.
    std::fs::write(&path, b"mine, later").unwrap();
    let ev = h.save(&path, true);
    match ev.as_slice() {
        [
            Event::VersionSaved { version: a, .. },
            Event::VersionSaved { version: b, .. },
            Event::Synced { pending: 0 },
        ] => {
            assert_eq!((a.number, b.number), (3, 4));
            assert_eq!(a.blob, rcad_cloud::blob_hash(b"mine"));
            assert_eq!(b.blob, rcad_cloud::blob_hash(b"mine, later"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn open_document_downloads_then_uses_the_cache_offline() {
    let server = FakeServer::start();
    let port = server.port;
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let doc = server_doc(&server, "Housing", b"housing v1");
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    let path = match ev.as_slice() {
        [
            Event::DocumentReady {
                path,
                link,
                read_only: false,
                from_cache: false,
            },
        ] => {
            assert!(path.starts_with(h.cache_root()), "{path:?}");
            assert_eq!(link.base_version, 1);
            assert_eq!(link.org, fake_server::ORG);
            path.clone()
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(std::fs::read(&path).unwrap(), b"housing v1");
    // Opening again does not download again (blob cache).
    let downloads = |s: &FakeServer| s.log().iter().filter(|l| l.ends_with("/content")).count();
    assert_eq!(downloads(&server), 1);
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::DocumentReady {
                from_cache: false,
                ..
            }]
        ),
        "{ev:?}"
    );
    assert_eq!(downloads(&server), 1);

    // A newer head replaces an unmodified working copy.
    server
        .state
        .lock()
        .unwrap()
        .save_as(&doc, "bob", b"housing v2");
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    assert!(
        matches!(&ev[..], [Event::DocumentReady { link, .. }] if link.base_version == 2),
        "{ev:?}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"housing v2");

    let _state = server.stop();
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    match ev.as_slice() {
        [
            Event::DocumentReady {
                path: p,
                read_only: false,
                from_cache: true,
                ..
            },
            Event::Offline { pending: 0 },
        ] => {
            assert_eq!(p, &std::fs::canonicalize(&path).unwrap());
        }
        other => panic!("{other:?}"),
    }
    let ev = h.run(Request::OpenDocument {
        document: "never-seen".into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [
                Event::Error {
                    request: "open_document",
                    ..
                },
                Event::Offline { .. }
            ]
        ),
        "{ev:?}"
    );
    let _ = port;
}

#[test]
fn open_document_keeps_a_modified_working_copy() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let doc = server_doc(&server, "Lever", b"lever v1");
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    let path = match ev.as_slice() {
        [Event::DocumentReady { path, .. }] => path.clone(),
        other => panic!("{other:?}"),
    };
    std::fs::write(&path, b"my local edit").unwrap();
    // Unchanged head: the modified copy is reported, nothing else.
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    assert!(
        matches!(ev.as_slice(), [Event::DocumentReady { .. }]),
        "{ev:?}"
    );
    server
        .state
        .lock()
        .unwrap()
        .save_as(&doc, "bob", b"lever v2");
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    match ev.as_slice() {
        [
            Event::DocumentReady { path: p, link, .. },
            Event::Conflict {
                base_version: 1,
                head_version: 2,
                ..
            },
        ] => {
            assert_eq!(p, &path);
            assert_eq!(link.base_version, 1);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"my local edit");
}

#[test]
fn hostile_document_names_stay_inside_the_cache() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    for name in [
        "../../etc/passwd",
        "C:\\x",
        "",
        "..",
        &"n".repeat(500),
        "日本語の部品",
    ] {
        let doc = server_doc(&server, name, b"x");
        let ev = h.run(Request::OpenDocument {
            document: doc.clone(),
        });
        let path = match ev.as_slice() {
            [Event::DocumentReady { path, .. }] => path.clone(),
            other => panic!("{name:?}: {other:?}"),
        };
        let root = std::fs::canonicalize(h.cache_root()).unwrap();
        assert!(
            std::fs::canonicalize(&path).unwrap().starts_with(&root),
            "{name:?} -> {path:?}"
        );
        let ev = h.run(Request::OpenVersion {
            document: doc,
            version: 1,
        });
        match ev.as_slice() {
            [
                Event::DocumentReady {
                    path,
                    read_only: true,
                    ..
                },
            ] => {
                assert!(
                    std::fs::canonicalize(path).unwrap().starts_with(&root),
                    "{path:?}"
                )
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn tampered_download_is_rejected() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let doc = server_doc(&server, "Tampered", b"real bytes");
    server.state.lock().unwrap().tamper_content = true;
    let ev = h.run(Request::OpenDocument {
        document: doc.clone(),
    });
    match ev.as_slice() {
        [
            Event::Error {
                request: "open_document",
                message,
                ..
            },
        ] => assert!(message.contains("hash"), "{message}"),
        other => panic!("{other:?}"),
    }
    let cache = h.cache();
    assert!(!cache.has_blob(&rcad_cloud::blob_hash(b"real bytes")));
    assert!(!cache.working_copy_path(&doc, "Tampered").exists());
    assert!(cache.link_for_document(&doc).unwrap().is_none());
}

#[test]
fn open_version_and_collaboration_requests() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Arm", b"arm v1");
    std::fs::write(&path, b"arm v2").unwrap();
    assert!(matches!(
        h.save(&path, false).as_slice(),
        [Event::VersionSaved { .. }]
    ));

    let ev = h.run(Request::OpenVersion {
        document: doc.clone(),
        version: 1,
    });
    match ev.as_slice() {
        [
            Event::DocumentReady {
                path: p,
                read_only: true,
                from_cache: false,
                link,
            },
        ] => {
            assert_eq!(std::fs::read(p).unwrap(), b"arm v1");
            assert!(
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("v1-Arm")
            );
            assert_eq!(link.base_version, 1);
        }
        other => panic!("{other:?}"),
    }
    // The read-only copy is not linked: the working copy keeps its link.
    assert_eq!(
        h.cache()
            .link_for_document(&doc)
            .unwrap()
            .unwrap()
            .base_version,
        2
    );
    let ev = h.run(Request::OpenVersion {
        document: doc.clone(),
        version: 9,
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                code: Some(ErrorCode::NotFound),
                ..
            }]
        ),
        "{ev:?}"
    );

    let ev = h.run(Request::SetMilestone {
        document: doc.clone(),
        version: 1,
        milestone: Some("First".into()),
    });
    match ev.as_slice() {
        [Event::Detail(d)] => assert_eq!(
            d.versions.last().unwrap().milestone.as_deref(),
            Some("First")
        ),
        other => panic!("{other:?}"),
    }
    let ev = h.run(Request::Promote {
        document: doc.clone(),
        version: 1,
    });
    match ev.as_slice() {
        [Event::Detail(d)] => {
            assert_eq!(d.document.head_version, 3);
            assert_eq!(d.versions[0].promoted_from, Some(1));
        }
        other => panic!("{other:?}"),
    }
    let ev = h.run(Request::CreateShare {
        document: doc.clone(),
        version: None,
        expires_in_days: Some(7),
        allow_download: true,
    });
    assert!(
        matches!(&ev[..], [Event::ShareCreated(r)] if r.link.allow_download),
        "{ev:?}"
    );
    let ev = h.run(Request::AddComment {
        document: doc.clone(),
        version: 2,
        body: "Fillet this".into(),
    });
    let cid = match ev.as_slice() {
        [Event::Comments { comments, .. }] if comments.len() == 1 => comments[0].id.clone(),
        other => panic!("{other:?}"),
    };
    let ev = h.run(Request::ResolveComment {
        document: doc.clone(),
        comment: cid,
        resolved: true,
    });
    assert!(
        matches!(&ev[..], [Event::Comments { comments, .. }] if comments[0].resolved),
        "{ev:?}"
    );
    let ev = h.run(Request::LoadComments {
        document: doc.clone(),
    });
    assert!(matches!(ev.as_slice(), [Event::Comments { .. }]), "{ev:?}");

    let ev = h.run(Request::ListProjects {
        org: fake_server::ORG.into(),
    });
    assert!(
        matches!(&ev[..], [Event::Projects { projects, .. }] if projects.len() == 1),
        "{ev:?}"
    );
    let ev = h.run(Request::CreateProject {
        org: fake_server::ORG.into(),
        name: "Jigs".into(),
    });
    assert!(
        matches!(&ev[..], [Event::Projects { projects, .. }] if projects.len() == 2),
        "{ev:?}"
    );
    let ev = h.run(Request::CreateFolder {
        project: PROJECT.into(),
        parent: None,
        name: "Fixtures".into(),
    });
    match ev.as_slice() {
        [Event::Tree(t)] => {
            assert_eq!(t.folders.len(), 1);
            assert_eq!(t.documents.len(), 1);
        }
        other => panic!("{other:?}"),
    }
    let ev = h.run(Request::LoadDetail {
        document: "missing".into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "load_detail",
                code: Some(ErrorCode::NotFound),
                ..
            }]
        ),
        "{ev:?}"
    );
}

#[test]
fn background_service_thread_answers_requests() {
    let server = FakeServer::start();
    let h = Harness::new();
    let svc = CloudService::start(h.config_path(), h.cache_root());
    let wait = |svc: &CloudService| {
        let t0 = Instant::now();
        loop {
            if let Some(e) = svc.poll() {
                return e;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "no event");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    svc.send(Request::Refresh);
    assert!(matches!(
        wait(&svc),
        Event::Error {
            request: "refresh",
            ..
        }
    ));
    svc.send(Request::SignIn {
        server_url: server.url.clone(),
        username: USER.into(),
        password: PASSWORD.into(),
    });
    assert!(matches!(wait(&svc), Event::SignedIn { .. }));
    svc.send(Request::LoadTree {
        project: PROJECT.into(),
    });
    assert!(matches!(wait(&svc), Event::Tree(_)));
    // Nothing was written before the sign-in except what sign-in stores.
    assert!(h.config_path().exists());
}

#[test]
fn nothing_touches_disk_or_network_without_an_account() {
    let mut h = Harness::new();
    for req in [
        Request::Refresh,
        Request::SyncPending,
        Request::OpenDocument {
            document: "d".into(),
        },
    ] {
        let ev = h.run(req);
        assert!(
            matches!(ev.as_slice(), [Event::Error { code: None, .. }]),
            "{ev:?}"
        );
    }
    assert!(!h.config_path().exists());
    assert!(!h.cache_root().exists());
}

#[test]
fn tls_is_configured_and_a_failed_handshake_counts_as_offline() {
    // A plain-HTTP listener answering an https:// URL: the TCP connect works and the TLS
    // handshake fails, which exercises the rustls + platform verifier setup without network.
    let server = FakeServer::start();
    let client =
        rcad_cloud::client::Client::new(&format!("https://127.0.0.1:{}", server.port)).unwrap();
    match client.info() {
        Err(rcad_cloud::CloudError::Offline(_)) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn member_and_organization_requests_answer_with_the_reloaded_list() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let ev = h.run(Request::LoadProjectMembers {
        project: PROJECT.into(),
    });
    assert_eq!(
        ev,
        vec![Event::ProjectMembers {
            project: PROJECT.into(),
            members: vec![]
        }]
    );
    let ev = h.run(Request::SetProjectMember {
        project: PROJECT.into(),
        username: "bob".into(),
        role: proto::ProjectRole::Editor,
    });
    match ev.as_slice() {
        [Event::ProjectMembers { project, members }] => {
            assert_eq!(project, PROJECT);
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].user.username, "bob");
            assert_eq!(members[0].role, proto::ProjectRole::Editor);
        }
        other => panic!("{other:?}"),
    }
    // Someone outside the organization cannot be added (the server answers 404).
    let ev = h.run(Request::SetProjectMember {
        project: PROJECT.into(),
        username: "carol".into(),
        role: proto::ProjectRole::Viewer,
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "set_project_member",
                code: Some(ErrorCode::NotFound),
                ..
            }]
        ),
        "{ev:?}"
    );
    let ev = h.run(Request::SetProjectMember {
        project: PROJECT.into(),
        username: "bob".into(),
        role: proto::ProjectRole::Viewer,
    });
    assert!(
        matches!(ev.as_slice(), [Event::ProjectMembers { members, .. }]
            if members.len() == 1 && members[0].role == proto::ProjectRole::Viewer),
        "{ev:?}"
    );
    let ev = h.run(Request::RemoveProjectMember {
        project: PROJECT.into(),
        username: "bob".into(),
    });
    assert!(
        matches!(ev.as_slice(), [Event::ProjectMembers { members, .. }] if members.is_empty()),
        "{ev:?}"
    );

    let ev = h.run(Request::LoadOrgMembers {
        org: fake_server::ORG.into(),
    });
    assert!(
        matches!(ev.as_slice(), [Event::OrgMembers { org, members }]
            if org == fake_server::ORG && members.len() == 2),
        "{ev:?}"
    );
    let ev = h.run(Request::SetOrgMember {
        org: fake_server::ORG.into(),
        username: "carol".into(),
        role: proto::OrgRole::Guest,
    });
    assert!(
        matches!(ev.as_slice(), [Event::OrgMembers { members, .. }]
            if members.iter().any(|m| m.user.username == "carol" && m.role == proto::OrgRole::Guest)),
        "{ev:?}"
    );
    let ev = h.run(Request::RemoveOrgMember {
        org: fake_server::ORG.into(),
        username: "carol".into(),
    });
    assert!(
        matches!(ev.as_slice(), [Event::OrgMembers { members, .. }] if members.len() == 2),
        "{ev:?}"
    );
    let ev = h.run(Request::LoadUsage {
        org: fake_server::ORG.into(),
    });
    match ev.as_slice() {
        [Event::Usage { org, usage }] => {
            assert_eq!(org, fake_server::ORG);
            assert_eq!((usage.used_bytes, usage.quota_bytes), (1_000, Some(10_000)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn document_and_folder_management_reload_the_tree() {
    let server = FakeServer::start();
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Bracket", b"one");
    h.upload("Plate", b"two");
    let ev = h.run(Request::CreateFolder {
        project: PROJECT.into(),
        parent: None,
        name: "Brackets".into(),
    });
    let folder = match ev.as_slice() {
        [Event::Tree(t)] => t.folders[0].id.clone(),
        other => panic!("{other:?}"),
    };
    let tree = |ev: Vec<Event>| match ev.as_slice() {
        [Event::Tree(t)] => t.clone(),
        other => panic!("{other:?}"),
    };

    let t = tree(h.run(Request::MoveDocument {
        document: doc.clone(),
        folder: Some(folder.clone()),
    }));
    let d = t.documents.iter().find(|d| d.id == doc).unwrap();
    assert_eq!(d.folder.as_deref(), Some(folder.as_str()));

    let t = tree(h.run(Request::RenameDocument {
        document: doc.clone(),
        name: "Wall bracket".into(),
    }));
    assert!(t.documents.iter().any(|d| d.name == "Wall bracket"));
    // The local link follows the new name.
    let link = h.cache().link_for_path(&path).unwrap().unwrap();
    assert_eq!(link.document_name, "Wall bracket");
    // Names are unique in a project.
    let ev = h.run(Request::RenameDocument {
        document: doc.clone(),
        name: "plate".into(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "rename_document",
                code: Some(ErrorCode::AlreadyExists),
                ..
            }]
        ),
        "{ev:?}"
    );

    // A folder with content cannot be deleted.
    let ev = h.run(Request::DeleteFolder {
        project: PROJECT.into(),
        folder: folder.clone(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [Event::Error {
                request: "delete_folder",
                code: Some(ErrorCode::NotEmpty),
                ..
            }]
        ),
        "{ev:?}"
    );
    let t = tree(h.run(Request::RenameFolder {
        project: PROJECT.into(),
        folder: folder.clone(),
        name: "Mounts".into(),
    }));
    assert_eq!(t.folders[0].name, "Mounts");

    let t = tree(h.run(Request::TrashDocument {
        project: PROJECT.into(),
        document: doc.clone(),
    }));
    assert!(t.documents.iter().all(|d| d.id != doc));
    assert!(t.trash.iter().any(|d| d.id == doc));
    // Still not empty: trashed documents keep their folder until restored.
    let ev = h.run(Request::DeleteFolder {
        project: PROJECT.into(),
        folder: folder.clone(),
    });
    assert!(matches!(ev.as_slice(), [Event::Error { .. }]), "{ev:?}");

    let t = tree(h.run(Request::RestoreDocument {
        document: doc.clone(),
    }));
    assert!(t.documents.iter().any(|d| d.id == doc));
    assert!(t.trash.is_empty());
    tree(h.run(Request::MoveDocument {
        document: doc.clone(),
        folder: None,
    }));
    let t = tree(h.run(Request::DeleteFolder {
        project: PROJECT.into(),
        folder,
    }));
    assert!(t.folders.is_empty());
}

#[test]
fn a_blocked_queue_entry_can_be_discarded() {
    let server = FakeServer::start();
    let port = server.port;
    let mut h = Harness::new();
    h.sign_in(&server.url);
    let (path, doc) = h.upload("Shaft", b"v1");
    let state = server.stop();
    std::fs::write(&path, b"v2").unwrap();
    assert_eq!(h.save(&path, false), vec![Event::Offline { pending: 1 }]);
    std::fs::write(&path, b"v3").unwrap();
    assert_eq!(h.save(&path, false), vec![Event::Offline { pending: 2 }]);

    // Back online, but the organization is over its quota: the queue stays blocked.
    state.lock().unwrap().fail_saves = Some((507, ErrorCode::QuotaExceeded));
    let server = FakeServer::start_on(port, state);
    let ev = h.run(Request::SyncPending);
    assert!(
        matches!(
            ev.as_slice(),
            [
                Event::Error {
                    request: "sync_pending",
                    code: Some(ErrorCode::QuotaExceeded),
                    ..
                },
                Event::Synced { pending: 2 }
            ]
        ),
        "{ev:?}"
    );
    let queue = match h.run(Request::LoadPending).as_slice() {
        [Event::PendingQueue { saves }] => saves.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0].blob, rcad_cloud::blob_hash(b"v2"));
    assert_eq!(queue[0].document, doc);

    // A stale listing (wrong blob at that position) discards nothing.
    let ev = h.run(Request::DiscardPending {
        index: 0,
        document: doc.clone(),
        blob: queue[1].blob.clone(),
    });
    assert!(
        matches!(
            ev.as_slice(),
            [
                Event::PendingQueue { saves },
                Event::Error {
                    request: "discard_pending",
                    ..
                }
            ] if saves.len() == 2
        ),
        "{ev:?}"
    );
    let ev = h.run(Request::DiscardPending {
        index: 0,
        document: doc.clone(),
        blob: queue[0].blob.clone(),
    });
    assert!(
        matches!(ev.as_slice(), [Event::PendingQueue { saves }]
            if saves.len() == 1 && saves[0].blob == rcad_cloud::blob_hash(b"v3")),
        "{ev:?}"
    );
    // The snapshot stays in the blob cache and the file is untouched.
    assert!(h.cache().has_blob(&rcad_cloud::blob_hash(b"v2")));
    assert_eq!(std::fs::read(&path).unwrap(), b"v3");

    server.state.lock().unwrap().fail_saves = None;
    let ev = h.run(Request::SyncPending);
    match ev.as_slice() {
        [
            Event::VersionSaved { version, .. },
            Event::Synced { pending: 0 },
        ] => {
            assert_eq!(version.number, 2);
            assert_eq!(version.blob, rcad_cloud::blob_hash(b"v3"));
        }
        other => panic!("{other:?}"),
    }
}
