//! Optional cloud tier through the real widgets, against a detached cloud
//! service: the test sees every request the UI sends and injects the events a
//! server would cause. Covers the signed-out Data panel, sign in, opening a
//! cloud document, the status chip (synced, modified, locked, offline,
//! conflict), auto-lock, Save Version, conflicts, share links, version
//! history, upload, errors in the message log and the 1280-px layout.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_cloud::cache::Link;
use rcad_cloud::proto;
use rcad_cloud::service::{CloudService, Event, Request};
use rcad_commands::Command;

struct Cloud {
    h: Harness<'static, RcadApp>,
    req: Receiver<Request>,
    ev: Sender<Event>,
    _dir: tempfile::TempDir,
    doc: PathBuf,
}

fn harness(size: [f32; 2]) -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size(size)
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn file_menu(h: &mut Harness<'_, RcadApp>, item: &str) {
    click(h, "File menu");
    click(h, item);
}

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn drain(rx: &Receiver<Request>) -> Vec<Request> {
    rx.try_iter().collect()
}

/// [`drain`] without the sign-in dialog's server info fetches (sent once the
/// typed URL has been stable for a moment, so their timing depends on the
/// machine's speed).
fn drain_no_info(rx: &Receiver<Request>) -> Vec<Request> {
    rx.try_iter()
        .filter(|r| !matches!(r, Request::LoadServerInfo { .. }))
        .collect()
}

fn me() -> proto::Me {
    proto::Me {
        user: proto::User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            server_admin: false,
        },
        orgs: vec![proto::OrgSummary {
            id: "o1".into(),
            name: "Home".into(),
            my_role: proto::OrgRole::Member,
        }],
    }
}

fn link(path: &Path, base: u64) -> Link {
    Link {
        local_path: path.to_path_buf(),
        server_url: "http://127.0.0.1:18640".into(),
        org: "o1".into(),
        project: "p1".into(),
        document: "d1".into(),
        document_name: "Bracket".into(),
        base_version: base,
        base_blob: None,
    }
}

fn version(n: u64, milestone: Option<&str>) -> proto::Version {
    proto::Version {
        number: n,
        blob: "a".repeat(64),
        size: 100,
        author: if n == 2 { "bob" } else { "alice" }.into(),
        created_at: 1_792_224_300 + n as i64 * 60,
        description: format!("change {n}"),
        milestone: milestone.map(String::from),
        promoted_from: None,
        generator: String::new(),
        thumbnail: None,
    }
}

fn summary(head: u64) -> proto::DocumentSummary {
    proto::DocumentSummary {
        id: "d1".into(),
        project: "p1".into(),
        folder: None,
        name: "Bracket".into(),
        head_version: head,
        updated_at: 1_792_224_300,
        updated_by: "alice".into(),
        lock: None,
        has_thumbnail: false,
    }
}

fn project() -> proto::Project {
    proto::Project {
        id: "p1".into(),
        org: "o1".into(),
        name: "Parts".into(),
        description: String::new(),
        my_role: proto::ProjectRole::Editor,
        created_at: 1_792_224_300,
        revision: 1,
    }
}

/// A signed-in app with a detached service (requests drained).
fn signed_in(size: [f32; 2]) -> Cloud {
    let (svc, req, ev) = CloudService::detached();
    let mut h = harness(size);
    let dir = tempfile::tempdir().unwrap();
    h.state_mut().attach_cloud_service(svc);
    assert_eq!(drain(&req), vec![Request::Refresh]);
    ev.send(Event::SignedIn {
        server_url: "http://127.0.0.1:18640".into(),
        server_name: "Home lab".into(),
        me: me(),
    })
    .unwrap();
    settle(&mut h);
    assert_eq!(
        drain(&req),
        vec![Request::ListProjects { org: "o1".into() }]
    );
    let doc = dir.path().join("Bracket.rcad.json");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bracket.rcad.json"),
        &doc,
    )
    .unwrap();
    Cloud {
        h,
        req,
        ev,
        _dir: dir,
        doc,
    }
}

/// [`signed_in`] with the example open as the working copy of cloud v3.
fn opened(size: [f32; 2]) -> Cloud {
    let mut c = signed_in(size);
    c.ev.send(Event::DocumentReady {
        path: c.doc.clone(),
        link: link(&c.doc, 3),
        read_only: false,
        from_cache: false,
    })
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    assert_eq!(c.h.state().summary().path.as_ref(), Some(&c.doc));
    c
}

fn chip(c: &Cloud) -> String {
    c.h.state().cloud_chip().unwrap_or_default()
}

fn edit(c: &mut Cloud) {
    let f = c.h.state().summary().features[0].id;
    c.h.state_mut().request(Command::RenameFeature {
        feature: f,
        name: "Edited outline".into(),
    });
    settle(&mut c.h);
    settle(&mut c.h);
    assert!(c.h.state().summary().dirty);
}

fn logged(c: &Cloud, text: &str) -> bool {
    c.h.state()
        .diagnostics()
        .iter()
        .any(|d| d.text.contains(text))
}

#[test]
fn without_an_account_nothing_starts_and_the_panel_offers_connect() {
    let mut h = harness([1272.0, 768.0]);
    let dir = tempfile::tempdir().unwrap();
    h.state_mut()
        .enable_cloud_at(dir.path().join("cloud.json"), dir.path().join("cache"));
    settle(&mut h);
    assert!(!h.state().cloud_service_running());
    assert!(h.state().cloud_chip().is_none());
    assert!(!h.state().cloud_panel_open());
    // Discoverable but quiet: the cloud commands are disabled with a reason.
    assert!(
        h.state()
            .command_enabled(rcad::shell::CmdId::SaveVersion)
            .is_err()
    );
    click(&mut h, "Data panel");
    assert!(h.state().cloud_panel_open());
    assert!(h.query_by_label_contains("works fully offline").is_some());
    click(&mut h, "Connect to server…");
    assert!(h.query_by_label("Server URL").is_some());
    // Esc cancels the dialog.
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.state().cloud_dialog().is_none());
    assert!(!h.state().cloud_service_running());
    assert!(!dir.path().join("cache").exists(), "no cache written");
}

#[test]
fn sign_in_dialog_sends_the_entered_fields_and_shows_errors() {
    let (svc, req, ev) = CloudService::detached();
    let mut h = harness([1400.0, 900.0]);
    h.state_mut().attach_cloud_service(svc);
    drain(&req);
    click(&mut h, "Data panel");
    click(&mut h, "Connect to server…");
    type_into(&mut h, "Server URL", "http://127.0.0.1:18640");
    type_into(&mut h, "Username", "alice");
    type_into(&mut h, "Password", "alice-pass-123");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(
        drain_no_info(&req),
        vec![Request::SignIn {
            server_url: "http://127.0.0.1:18640".into(),
            username: "alice".into(),
            password: "alice-pass-123".into(),
            allow_insecure_http: false,
        }]
    );
    ev.send(Event::Error {
        request: "sign_in",
        code: Some(proto::ErrorCode::Unauthenticated),
        message: "wrong username or password".into(),
    })
    .unwrap();
    settle(&mut h);
    assert!(h.query_by_label("wrong username or password").is_some());
    assert!(
        h.state()
            .diagnostics()
            .iter()
            .any(|d| d.text == "Cloud: wrong username or password"),
        "{:?}",
        h.state().diagnostics()
    );
    assert!(
        !h.state()
            .diagnostics()
            .iter()
            .any(|d| d.text.contains("alice-pass-123")),
        "password never logged"
    );
    ev.send(Event::SignedIn {
        server_url: "http://127.0.0.1:18640".into(),
        server_name: "Home lab".into(),
        me: me(),
    })
    .unwrap();
    settle(&mut h);
    assert!(h.state().cloud_dialog().is_none());
    assert!(h.query_by_label("Sign out").is_some());
    assert_eq!(
        drain_no_info(&req),
        vec![Request::ListProjects { org: "o1".into() }]
    );
    click(&mut h, "Sign out");
    assert_eq!(drain(&req), vec![Request::SignOut]);
}

#[test]
fn plain_http_to_a_public_host_needs_the_unencrypted_checkbox() {
    let (svc, req, _ev) = CloudService::detached();
    let mut h = harness([1400.0, 900.0]);
    h.state_mut().attach_cloud_service(svc);
    drain(&req);
    click(&mut h, "Data panel");
    click(&mut h, "Connect to server…");
    // A local address shows no warning and needs no consent.
    type_into(&mut h, "Server URL", "http://192.168.1.20:8080");
    settle(&mut h);
    assert!(h.query_by_label("Allow unencrypted connection").is_none());
    type_into(&mut h, "Server URL", "http://cad.example");
    type_into(&mut h, "Username", "alice");
    type_into(&mut h, "Password", "alice-pass-123");
    settle(&mut h);
    assert!(h.query_by_label("Allow unencrypted connection").is_some());
    assert!(
        h.query_by_label_contains("travel unencrypted").is_some(),
        "the warning is shown"
    );
    // Without the tick nothing is sent.
    click(&mut h, "Connect");
    assert!(drain_no_info(&req).is_empty());
    assert!(
        h.query_by_label_contains("would not be encrypted")
            .is_some(),
        "explains why"
    );
    click(&mut h, "Allow unencrypted connection");
    click(&mut h, "Connect");
    assert_eq!(
        drain_no_info(&req),
        vec![Request::SignIn {
            server_url: "http://cad.example".into(),
            username: "alice".into(),
            password: "alice-pass-123".into(),
            allow_insecure_http: true,
        }]
    );
}

fn server_info(local_login: bool, sso: bool) -> proto::ServerInfo {
    proto::ServerInfo {
        product: proto::PRODUCT.into(),
        version: "test".into(),
        protocol: proto::PROTOCOL_VERSION,
        name: "Home lab".into(),
        local_login,
        org_creation: false,
        max_blob_bytes: 1 << 30,
        oidc: sso.then(|| proto::OidcInfo {
            display_name: "Pocket ID".into(),
            authorization_endpoint: "https://id.example/authorize".into(),
            client_id: "rcad".into(),
            scopes: "openid profile".into(),
        }),
    }
}

/// The sign-in dialog with `url` typed and its info fetch answered.
fn sign_in_with_info(
    info: proto::ServerInfo,
) -> (Harness<'static, RcadApp>, Receiver<Request>, Sender<Event>) {
    let (svc, req, ev) = CloudService::detached();
    let mut h = harness([1400.0, 900.0]);
    h.state_mut().attach_cloud_service(svc);
    h.state_mut().set_cloud_info_debounce(Duration::ZERO);
    drain(&req);
    click(&mut h, "Data panel");
    click(&mut h, "Connect to server…");
    type_into(&mut h, "Server URL", "https://cad.example");
    settle(&mut h);
    let sent = drain(&req);
    assert_eq!(
        sent.last(),
        Some(&Request::LoadServerInfo {
            server_url: "https://cad.example".into()
        }),
        "{sent:?}"
    );
    // Each URL is asked once, not on every frame.
    assert!(
        sent.iter()
            .filter(|r| **r
                == Request::LoadServerInfo {
                    server_url: "https://cad.example".into()
                })
            .count()
            == 1,
        "{sent:?}"
    );
    ev.send(Event::ServerInfo {
        server_url: "https://cad.example".into(),
        info,
    })
    .unwrap();
    settle(&mut h);
    (h, req, ev)
}

#[test]
fn a_server_with_single_sign_on_offers_its_button_and_the_browser_flow() {
    let (mut h, req, ev) = sign_in_with_info(server_info(true, true));
    assert!(h.query_by_label("Sign in with Pocket ID").is_some());
    assert!(
        h.query_by_label("Password").is_some(),
        "password sign-in stays"
    );
    assert_eq!(h.state().cloud_probe()["sso_available"], true);
    click(&mut h, "Sign in with Pocket ID");
    assert_eq!(
        drain(&req),
        vec![Request::SignInSso {
            server_url: "https://cad.example".into(),
            allow_insecure_http: false,
        }]
    );
    assert!(h.query_by_label("Waiting for the browser…").is_some());
    let url = "https://id.example/authorize?response_type=code&state=abc";
    ev.send(Event::OpenBrowser { url: url.into() }).unwrap();
    settle(&mut h);
    // Recorded and shown for copying; no browser is spawned in tests.
    assert_eq!(h.state().cloud_sso_url(), Some(url));
    assert_eq!(h.state().cloud_probe()["sso_waiting"], true);
    assert!(h.query_by_label("Open the page again").is_some());
    // A refusal shows in the dialog and ends the wait.
    ev.send(Event::Error {
        request: "sign_in_sso",
        code: None,
        message: "the identity provider refused the sign-in: access_denied".into(),
    })
    .unwrap();
    settle(&mut h);
    assert!(
        h.query_by_label("the identity provider refused the sign-in: access_denied")
            .is_some()
    );
    assert!(h.query_by_label("Waiting for the browser…").is_none());
    assert_eq!(h.state().cloud_probe()["sso_waiting"], false);
    // Again, and this time it works.
    click(&mut h, "Sign in with Pocket ID");
    drain(&req);
    ev.send(Event::OpenBrowser { url: url.into() }).unwrap();
    ev.send(Event::SignedIn {
        server_url: "https://cad.example".into(),
        server_name: "Home lab".into(),
        me: me(),
    })
    .unwrap();
    settle(&mut h);
    assert!(h.state().cloud_dialog().is_none());
    assert_eq!(h.state().cloud_probe()["sso_waiting"], false);
}

#[test]
fn cancelling_a_single_sign_on_tells_the_service() {
    let (mut h, req, ev) = sign_in_with_info(server_info(true, true));
    click(&mut h, "Sign in with Pocket ID");
    drain(&req);
    ev.send(Event::OpenBrowser {
        url: "https://id.example/authorize?x=1".into(),
    })
    .unwrap();
    settle(&mut h);
    click(&mut h, "Cancel");
    assert_eq!(drain(&req), vec![Request::CancelSso]);
    assert!(h.state().cloud_dialog().is_none());
    // Closing the dialog when nothing is waiting sends nothing.
    click(&mut h, "Connect to server…");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(drain_no_info(&req).is_empty());
}

#[test]
fn a_server_without_password_sign_in_shows_only_single_sign_on() {
    let (h, _req, _ev) = sign_in_with_info(server_info(false, true));
    assert!(h.query_by_label("Password").is_none());
    assert!(h.query_by_label("Username").is_none());
    assert!(h.query_by_label("Connect").is_none());
    assert!(h.query_by_label("Sign in with Pocket ID").is_some());
    assert_eq!(h.state().cloud_probe()["password_sign_in"], false);
}

#[test]
fn a_server_without_single_sign_on_shows_the_password_form_only() {
    let (mut h, req, ev) = sign_in_with_info(server_info(true, false));
    assert!(h.query_by_label_contains("Sign in with").is_none());
    assert!(h.query_by_label("Password").is_some());
    // An unreachable server leaves a quiet note, not a banner.
    type_into(&mut h, "Server URL", "https://other.example");
    settle(&mut h);
    assert_eq!(
        drain(&req).last(),
        Some(&Request::LoadServerInfo {
            server_url: "https://other.example".into()
        })
    );
    ev.send(Event::Error {
        request: "load_server_info",
        code: None,
        message: "server unreachable: dns".into(),
    })
    .unwrap();
    settle(&mut h);
    assert!(
        h.query_by_label_contains("Could not read this server's sign-in options")
            .is_some()
    );
    assert!(h.query_by_label("Password").is_some());
}

#[test]
fn document_ready_opens_and_edits_lock_then_save_version_unlocks() {
    let mut c = opened([1400.0, 900.0]);
    assert_eq!(chip(&c), "☁ Bracket v3 · Synced");
    assert!(c.h.query_by_label("Cloud status").is_some());
    edit(&mut c);
    assert_eq!(chip(&c), "☁ Bracket v3 · Modified");
    assert_eq!(
        drain(&c.req),
        vec![Request::Lock {
            document: "d1".into()
        }]
    );
    c.ev.send(Event::LockAcquired {
        document: "d1".into(),
        lock: proto::Lock {
            holder: "alice".into(),
            since: 0,
            expires_at: 600,
        },
    })
    .unwrap();
    settle(&mut c.h);
    file_menu(&mut c.h, "Save Version to Cloud…");
    type_into(&mut c.h, "Version description", "wider flange");
    type_into(&mut c.h, "Milestone name (optional)", "Prototype A");
    click(&mut c.h, "Save Version");
    settle(&mut c.h);
    // Saved locally first (the document was dirty), then the version.
    assert!(!c.h.state().summary().dirty);
    assert_eq!(
        drain(&c.req),
        vec![Request::SaveVersion {
            local_path: c.doc.clone(),
            description: "wider flange".into(),
            milestone: Some("Prototype A".into()),
            force: false,
            thumbnail: None,
        }]
    );
    c.ev.send(Event::VersionSaved {
        link: link(&c.doc, 4),
        version: version(4, Some("Prototype A")),
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v4 · Synced");
    assert_eq!(
        drain(&c.req),
        vec![Request::Unlock {
            document: "d1".into()
        }]
    );
    assert!(logged(&c, "Cloud: saved version 4 of Bracket"));
}

#[test]
fn auto_lock_off_sends_no_lock() {
    let mut c = opened([1400.0, 900.0]);
    click(&mut c.h, "Preferences");
    click(&mut c.h, "Pref cloud auto-lock");
    edit(&mut c);
    assert_eq!(chip(&c), "☁ Bracket v3 · Modified");
    assert!(drain(&c.req).is_empty());
}

#[test]
fn conflict_modal_buttons_send_the_right_requests() {
    let mut c = opened([1400.0, 900.0]);
    let conflict = |c: &mut Cloud| {
        c.ev.send(Event::Conflict {
            local_path: c.doc.clone(),
            document: "d1".into(),
            base_version: 3,
            head_version: 4,
        })
        .unwrap();
        settle(&mut c.h);
        assert_eq!(chip(c), "☁ Bracket v3 · Conflict");
        assert!(
            c.h.query_by_label_contains("Version 4 was saved by someone else after version 3")
                .is_some()
        );
    };
    file_menu(&mut c.h, "Save Version to Cloud…");
    type_into(&mut c.h, "Version description", "mine");
    click(&mut c.h, "Save Version");
    drain(&c.req);
    conflict(&mut c);
    click(&mut c.h, "Save as version 5 anyway");
    assert_eq!(
        drain(&c.req),
        vec![Request::SaveVersion {
            local_path: c.doc.clone(),
            description: "mine".into(),
            milestone: None,
            force: true,
            thumbnail: None,
        }]
    );
    assert!(c.h.state().cloud_conflict().is_none());
    conflict(&mut c);
    click(&mut c.h, "Save as new document…");
    assert_eq!(
        drain(&c.req),
        vec![Request::UploadNew {
            local_path: c.doc.clone(),
            project: "p1".into(),
            folder: None,
            name: "Bracket (copy)".into(),
            thumbnail: None,
        }]
    );
    conflict(&mut c);
    click(&mut c.h, "Cancel");
    assert!(drain(&c.req).is_empty());
    assert!(c.h.state().cloud_conflict().is_none());
    assert_eq!(chip(&c), "☁ Bracket v3 · Synced");
}

#[test]
fn locked_offline_and_synced_update_the_chip() {
    let mut c = opened([1400.0, 900.0]);
    c.ev.send(Event::Locked {
        document: "d1".into(),
        lock: proto::Lock {
            holder: "bob".into(),
            since: 0,
            expires_at: 600,
        },
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · Locked by bob");
    assert!(logged(&c, "locked by bob"));
    // Local editing is never blocked by someone else's lock.
    edit(&mut c);
    assert!(
        drain(&c.req).is_empty(),
        "no lock request while bob holds it"
    );
    c.ev.send(Event::LockReleased {
        document: "d1".into(),
    })
    .unwrap();
    c.ev.send(Event::Offline { pending: 2 }).unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · Offline · 2 pending");
    file_menu(&mut c.h, "Sync Now");
    assert_eq!(drain(&c.req), vec![Request::SyncPending]);
    c.ev.send(Event::Synced { pending: 0 }).unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · Modified");
    assert!(
        c.h.state()
            .command_enabled(rcad::shell::CmdId::SyncNow)
            .is_err()
    );
}

#[test]
fn share_dialog_creates_a_link_and_shows_the_url() {
    let mut c = opened([1400.0, 900.0]);
    file_menu(&mut c.h, "Share Link…");
    assert_eq!(
        drain(&c.req),
        vec![
            Request::LoadShares {
                document: "d1".into()
            },
            Request::LoadDetail {
                document: "d1".into()
            }
        ]
    );
    click(&mut c.h, "Create link");
    assert_eq!(
        drain(&c.req),
        vec![Request::CreateShare {
            document: "d1".into(),
            version: None,
            expires_in_days: Some(7),
            allow_download: true,
        }]
    );
    let url = "http://127.0.0.1:18640/s/tok123";
    c.ev.send(Event::ShareCreated(proto::CreateShareResponse {
        url: url.into(),
        token: "tok123".into(),
        link: proto::ShareLink {
            id: "s1".into(),
            document: "d1".into(),
            version: None,
            created_by: "alice".into(),
            created_at: 0,
            expires_at: None,
            allow_download: true,
        },
    }))
    .unwrap();
    settle(&mut c.h);
    assert_eq!(c.h.state().cloud_share_url(), Some(url));
    assert_eq!(c.h.get_by_label("Share URL").value().as_deref(), Some(url));
    assert!(c.h.query_by_label("Copy").is_some());
    // The list of links is reloaded to include the new one.
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadShares {
            document: "d1".into()
        }]
    );
}

#[test]
fn version_history_lists_versions_and_promote_asks_first() {
    let mut c = opened([1400.0, 900.0]);
    file_menu(&mut c.h, "Version History");
    assert!(c.h.state().cloud_panel_open());
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadDetail {
            document: "d1".into()
        }]
    );
    c.ev.send(Event::Detail(proto::DocumentDetail {
        document: summary(3),
        versions: vec![
            version(3, None),
            version(2, Some("Prototype A")),
            version(1, None),
        ],
        my_role: proto::ProjectRole::Editor,
    }))
    .unwrap();
    settle(&mut c.h);
    assert_eq!(c.h.state().cloud_probe()["versions_listed"], 3);
    assert_eq!(c.h.state().cloud_probe()["detail_head"], 3);
    assert_eq!(c.h.state().cloud_probe()["milestones_listed"], 1);
    assert!(c.h.query_by_label("★ v2").is_some());
    assert!(c.h.query_by_label("Milestone: Prototype A").is_some());
    assert!(c.h.query_by_label("change 2").is_some());
    // The head cannot be promoted.
    assert!(
        c.h.get_by_label("Promote v3")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut c.h, "Open v2");
    assert_eq!(
        drain(&c.req),
        vec![Request::OpenVersion {
            document: "d1".into(),
            version: 2
        }]
    );
    click(&mut c.h, "Promote v1");
    assert!(drain(&c.req).is_empty(), "asks first");
    assert!(c.h.query_by_label_contains("saved as version 4").is_some());
    click(&mut c.h, "Promote");
    assert_eq!(
        drain(&c.req),
        vec![Request::Promote {
            document: "d1".into(),
            version: 1
        }]
    );
    click(&mut c.h, "Clear milestone v2");
    assert_eq!(
        drain(&c.req),
        vec![Request::SetMilestone {
            document: "d1".into(),
            version: 2,
            milestone: None
        }]
    );
}

#[test]
fn read_only_copy_disables_save_version() {
    let mut c = signed_in([1400.0, 900.0]);
    c.ev.send(Event::DocumentReady {
        path: c.doc.clone(),
        link: link(&c.doc, 2),
        read_only: true,
        from_cache: false,
    })
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v2 · Read-only");
    assert!(logged(&c, "read-only copy of v2; Save Version is disabled"));
    assert!(
        c.h.state()
            .command_enabled(rcad::shell::CmdId::SaveVersion)
            .is_err()
    );
    assert_eq!(c.h.state().cloud_probe()["state"], "read_only");
    assert_eq!(c.h.state().cloud_probe()["save_version_enabled"], false);
    edit(&mut c);
    assert!(drain(&c.req).is_empty(), "no lock for a read-only copy");
}

#[test]
fn browse_projects_open_and_upload() {
    let mut c = signed_in([1400.0, 900.0]);
    click(&mut c.h, "Data panel");
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Project Parts");
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadTree {
            project: "p1".into()
        }]
    );
    c.ev.send(Event::Tree(proto::ProjectTree {
        project: project(),
        folders: vec![proto::Folder {
            id: "f1".into(),
            project: "p1".into(),
            parent: None,
            name: "Brackets".into(),
        }],
        documents: vec![summary(3)],
        trash: vec![],
    }))
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Document Bracket");
    click(&mut c.h, "Open");
    assert_eq!(
        drain(&c.req),
        vec![Request::OpenDocument {
            document: "d1".into()
        }]
    );
    // Upload a local (unlinked) document; the name defaults to the file stem.
    c.h.state_mut().request(Command::Open {
        path: c.doc.clone(),
        safe_mode: false,
    });
    settle(&mut c.h);
    click(&mut c.h, "Upload current document…");
    assert_eq!(
        c.h.get_by_label("Document name").value().as_deref(),
        Some("Bracket")
    );
    click(&mut c.h, "Upload");
    assert_eq!(
        drain(&c.req),
        vec![Request::UploadNew {
            local_path: c.doc.clone(),
            project: "p1".into(),
            folder: None,
            name: "Bracket".into(),
            thumbnail: None,
        }]
    );
    click(&mut c.h, "New folder…");
    type_into(&mut c.h, "Folder name", "Clips");
    click(&mut c.h, "Create folder");
    assert_eq!(
        drain(&c.req),
        vec![Request::CreateFolder {
            project: "p1".into(),
            parent: None,
            name: "Clips".into(),
        }]
    );
}

#[test]
fn errors_land_in_the_message_log() {
    let mut c = signed_in([1400.0, 900.0]);
    c.ev.send(Event::Error {
        request: "save_version",
        code: Some(proto::ErrorCode::QuotaExceeded),
        message: "storage quota exceeded".into(),
    })
    .unwrap();
    c.ev.send(Event::Error {
        request: "refresh",
        code: Some(proto::ErrorCode::Unauthenticated),
        message: "token expired".into(),
    })
    .unwrap();
    settle(&mut c.h);
    let d = c.h.state().diagnostics();
    assert!(
        d.iter()
            .any(|d| d.error && d.text == "Cloud: storage quota exceeded"),
        "{d:?}"
    );
    // Background failures are logged without an error banner.
    assert!(
        d.iter()
            .any(|d| !d.error
                && d.text == "Cloud: token expired (sign in again from the Data panel)")
    );
}

#[test]
fn layout_at_1280_with_the_data_panel_and_chip() {
    let mut c = opened([1272.0, 768.0]);
    let (x0, w0) = {
        let r = c.h.state().view_rect().unwrap();
        (r.min.x, r.width())
    };
    click(&mut c.h, "Data panel");
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Project Parts");
    c.ev.send(Event::Tree(proto::ProjectTree {
        project: project(),
        folders: vec![],
        documents: vec![summary(3)],
        trash: vec![],
    }))
    .unwrap();
    edit(&mut c);
    settle(&mut c.h);
    let view = c.h.state().view_rect().unwrap();
    assert!(view.min.x > x0, "the panel sits left of the browser");
    assert!(
        view.width() >= 380.0,
        "viewport too narrow: {} (was {w0})",
        view.width()
    );
    // One application-bar row, the chip inside the window and clear of the tabs.
    let rect = |l: &str| c.h.get_by_label(l).rect();
    let chip = rect("Cloud status");
    for l in ["Data panel", "File menu", "UTILITIES tab", "Preferences"] {
        assert!(
            (rect(l).center().y - chip.center().y).abs() < 2.0,
            "{l} is not on the app-bar row"
        );
    }
    assert!(
        chip.min.x > rect("UTILITIES tab").max.x,
        "chip overlaps the tabs"
    );
    assert!(chip.max.x <= 1272.0);
    let doc_row = rect("Document Bracket");
    assert!(
        doc_row.max.x <= view.min.x,
        "panel content overflows into the browser"
    );
}

fn user(name: &str) -> proto::User {
    proto::User {
        id: format!("u-{name}"),
        username: name.into(),
        display_name: name.into(),
        server_admin: false,
    }
}

fn folder(id: &str, name: &str, parent: Option<&str>) -> proto::Folder {
    proto::Folder {
        id: id.into(),
        project: "p1".into(),
        parent: parent.map(String::from),
        name: name.into(),
    }
}

/// Panel open on project Parts (with `role`) and `tree` shown.
fn browsing(c: &mut Cloud, role: proto::ProjectRole, tree: proto::ProjectTree) {
    click(&mut c.h, "Data panel");
    let p = proto::Project {
        my_role: role,
        ..project()
    };
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![p.clone()],
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Project Parts");
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadTree {
            project: "p1".into()
        }]
    );
    c.ev.send(Event::Tree(proto::ProjectTree { project: p, ..tree }))
        .unwrap();
    settle(&mut c.h);
}

fn tree(folders: Vec<proto::Folder>, docs: Vec<proto::DocumentSummary>) -> proto::ProjectTree {
    proto::ProjectTree {
        project: project(),
        folders,
        documents: docs,
        trash: vec![],
    }
}

fn probe(c: &Cloud) -> serde_json::Value {
    c.h.state().cloud_probe()
}

#[test]
fn members_dialog_edits_roles_and_explains_refusals_inline() {
    let mut c = signed_in([1400.0, 900.0]);
    browsing(&mut c, proto::ProjectRole::Admin, tree(vec![], vec![]));
    click(&mut c.h, "Members…");
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadProjectMembers {
            project: "p1".into()
        }]
    );
    assert_eq!(probe(&c)["dialog_kind"], "members");
    c.ev.send(Event::ProjectMembers {
        project: "p1".into(),
        members: vec![proto::ProjectMember {
            user: user("bob"),
            role: proto::ProjectRole::Editor,
        }],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["members_listed"], 1);
    assert_eq!(probe(&c)["members"][0]["role"], "Editor");
    assert_eq!(probe(&c)["member_roles"]["bob"], "Editor");

    // Adding someone outside the organization: the server's 404 is explained in the window.
    type_into(&mut c.h, "Member username", "carol");
    click(&mut c.h, "Add member");
    assert_eq!(
        drain(&c.req),
        vec![Request::SetProjectMember {
            project: "p1".into(),
            username: "carol".into(),
            role: proto::ProjectRole::Editor,
        }]
    );
    c.ev.send(Event::Error {
        request: "set_project_member",
        code: Some(proto::ErrorCode::NotFound),
        message: "no such user in the organization".into(),
    })
    .unwrap();
    settle(&mut c.h);
    let err = probe(&c)["admin_error"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(err.contains("member of the organization first"), "{err}");
    assert_eq!(probe(&c)["admin_error_code"], "not_found");
    assert!(
        c.h.query_by_label_contains("member of the organization first")
            .is_some()
    );
    assert!(
        !c.h.state().diagnostics().iter().any(|d| d.error),
        "shown in the window, not as an error banner"
    );

    // Change bob's role and remove him.
    click(&mut c.h, "Role of bob");
    click(&mut c.h, "Viewer");
    assert_eq!(
        drain(&c.req),
        vec![Request::SetProjectMember {
            project: "p1".into(),
            username: "bob".into(),
            role: proto::ProjectRole::Viewer,
        }]
    );
    assert!(probe(&c)["admin_error"].is_null(), "a new change clears it");
    assert!(probe(&c)["admin_error_code"].is_null());
    click(&mut c.h, "Remove bob");
    assert_eq!(
        drain(&c.req),
        vec![Request::RemoveProjectMember {
            project: "p1".into(),
            username: "bob".into(),
        }]
    );
    click(&mut c.h, "Close members");
    assert!(c.h.state().cloud_dialog().is_none());
}

#[test]
fn members_are_read_only_below_project_admin() {
    let mut c = signed_in([1400.0, 900.0]);
    browsing(&mut c, proto::ProjectRole::Editor, tree(vec![], vec![]));
    click(&mut c.h, "Members…");
    c.ev.send(Event::ProjectMembers {
        project: "p1".into(),
        members: vec![proto::ProjectMember {
            user: user("bob"),
            role: proto::ProjectRole::Editor,
        }],
    })
    .unwrap();
    settle(&mut c.h);
    assert!(c.h.query_by_label("Role of bob").is_none());
    assert!(c.h.query_by_label("Remove bob").is_none());
    assert!(c.h.query_by_label("Add member").is_none());
    assert!(
        c.h.query_by_label_contains("Only project administrators")
            .is_some()
    );
    // An organization member who is not an admin sees no Organization window.
    assert!(c.h.query_by_label("Organization…").is_none());
}

#[test]
fn organization_window_for_owners_lists_members_and_storage() {
    let mut c = signed_in([1400.0, 900.0]);
    let mut owner = me();
    owner.orgs[0].my_role = proto::OrgRole::Owner;
    c.ev.send(Event::SignedIn {
        server_url: "http://127.0.0.1:18640".into(),
        server_name: "Home lab".into(),
        me: owner,
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Data panel");
    drain(&c.req);
    click(&mut c.h, "Organization…");
    assert_eq!(
        drain(&c.req),
        vec![
            Request::LoadOrgMembers { org: "o1".into() },
            Request::LoadUsage { org: "o1".into() },
            Request::LoadOrgActivity { org: "o1".into() }
        ]
    );
    c.ev.send(Event::OrgMembers {
        org: "o1".into(),
        members: vec![
            proto::OrgMember {
                user: user("alice"),
                role: proto::OrgRole::Owner,
            },
            proto::OrgMember {
                user: user("bob"),
                role: proto::OrgRole::Member,
            },
        ],
    })
    .unwrap();
    c.ev.send(Event::Usage {
        org: "o1".into(),
        usage: proto::OrgUsage {
            used_bytes: 2_500_000,
            quota_bytes: Some(10_000_000),
            documents: 3,
            versions: 9,
        },
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["org_members_listed"], 2);
    assert_eq!(probe(&c)["usage_bytes"], 2_500_000);
    assert!(c.h.query_by_label("Storage usage").is_some());
    assert!(
        c.h.query_by_label_contains("3 documents, 9 versions")
            .is_some()
    );
    click(&mut c.h, "Organization role of bob");
    click(&mut c.h, "Admin");
    assert_eq!(
        drain(&c.req),
        vec![Request::SetOrgMember {
            org: "o1".into(),
            username: "bob".into(),
            role: proto::OrgRole::Admin,
        }]
    );
    type_into(&mut c.h, "Organization username", "dave");
    click(&mut c.h, "Add to organization");
    assert_eq!(
        drain(&c.req),
        vec![Request::SetOrgMember {
            org: "o1".into(),
            username: "dave".into(),
            role: proto::OrgRole::Member,
        }]
    );
    c.ev.send(Event::Error {
        request: "set_org_member",
        code: Some(proto::ErrorCode::NotFound),
        message: "no such user".into(),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(
        probe(&c)["admin_error"]
            .as_str()
            .is_some_and(|e| e.contains("account on this server"))
    );
    click(&mut c.h, "Remove bob from the organization");
    assert_eq!(
        drain(&c.req),
        vec![Request::RemoveOrgMember {
            org: "o1".into(),
            username: "bob".into(),
        }]
    );
}

#[test]
fn folders_are_upload_targets_and_documents_rename_move_and_trash() {
    let mut c = signed_in([1400.0, 900.0]);
    browsing(
        &mut c,
        proto::ProjectRole::Editor,
        tree(vec![folder("f1", "Brackets", None)], vec![summary(3)]),
    );
    assert_eq!(probe(&c)["document_folders"]["Bracket"], "/");
    // Selecting a folder makes it the target of New folder and Upload.
    click(&mut c.h, "Folder Brackets");
    assert_eq!(probe(&c)["selected_folder"], "f1");
    click(&mut c.h, "New folder…");
    type_into(&mut c.h, "Folder name", "Clips");
    click(&mut c.h, "Create folder");
    assert_eq!(
        drain(&c.req),
        vec![Request::CreateFolder {
            project: "p1".into(),
            parent: Some("f1".into()),
            name: "Clips".into(),
        }]
    );
    c.h.state_mut().request(Command::Open {
        path: c.doc.clone(),
        safe_mode: false,
    });
    settle(&mut c.h);
    click(&mut c.h, "Upload current document…");
    click(&mut c.h, "Upload");
    assert_eq!(
        drain(&c.req),
        vec![Request::UploadNew {
            local_path: c.doc.clone(),
            project: "p1".into(),
            folder: Some("f1".into()),
            name: "Bracket".into(),
            thumbnail: None,
        }]
    );
    click(&mut c.h, "Rename folder…");
    type_into(&mut c.h, "New folder name", "Mounts");
    click(&mut c.h, "Rename folder");
    assert_eq!(
        drain(&c.req),
        vec![Request::RenameFolder {
            project: "p1".into(),
            folder: "f1".into(),
            name: "Mounts".into(),
        }]
    );
    click(&mut c.h, "Delete folder…");
    click(&mut c.h, "Delete folder");
    assert_eq!(
        drain(&c.req),
        vec![Request::DeleteFolder {
            project: "p1".into(),
            folder: "f1".into(),
        }]
    );
    // Clicking the selected folder again clears the target.
    click(&mut c.h, "Folder Brackets");
    assert!(probe(&c)["selected_folder"].is_null());

    click(&mut c.h, "Document Bracket");
    click(&mut c.h, "Rename…");
    type_into(&mut c.h, "New name", "Wall bracket");
    click(&mut c.h, "Rename");
    assert_eq!(
        drain(&c.req),
        vec![Request::RenameDocument {
            document: "d1".into(),
            name: "Wall bracket".into(),
        }]
    );
    click(&mut c.h, "Move…");
    click(&mut c.h, "Move to");
    click(&mut c.h, "Brackets");
    click(&mut c.h, "Move");
    assert_eq!(
        drain(&c.req),
        vec![Request::MoveDocument {
            document: "d1".into(),
            folder: Some("f1".into()),
        }]
    );
    click(&mut c.h, "Move to trash…");
    assert_eq!(probe(&c)["dialog_kind"], "trash_document");
    click(&mut c.h, "Move to trash");
    assert_eq!(
        drain(&c.req),
        vec![Request::TrashDocument {
            project: "p1".into(),
            document: "d1".into(),
        }]
    );
    // The reload shows it in the trash; Restore brings it back.
    c.ev.send(Event::Tree(proto::ProjectTree {
        trash: vec![summary(3)],
        ..tree(vec![folder("f1", "Brackets", None)], vec![])
    }))
    .unwrap();
    settle(&mut c.h);
    assert!(probe(&c)["selected_document"].is_null());
    assert_eq!(probe(&c)["trash_listed"], 1);
    click(&mut c.h, "Trash");
    click(&mut c.h, "Restore Bracket");
    assert_eq!(
        drain(&c.req),
        vec![Request::RestoreDocument {
            document: "d1".into()
        }]
    );
}

#[test]
fn viewers_get_no_document_management() {
    let mut c = signed_in([1400.0, 900.0]);
    browsing(
        &mut c,
        proto::ProjectRole::Viewer,
        tree(vec![folder("f1", "Brackets", None)], vec![summary(3)]),
    );
    click(&mut c.h, "Document Bracket");
    for l in ["Rename…", "Move…", "Move to trash…", "New folder…"] {
        assert!(
            c.h.get_by_label(l).accesskit_node().is_disabled(),
            "{l} is enabled for a viewer"
        );
    }
    // Members are visible to everyone with view rights.
    assert!(!c.h.get_by_label("Members…").accesskit_node().is_disabled());
}

#[test]
fn queued_versions_are_listed_and_discarded_after_confirmation() {
    let mut c = signed_in([1400.0, 900.0]);
    click(&mut c.h, "Data panel");
    c.ev.send(Event::Offline { pending: 2 }).unwrap();
    settle(&mut c.h);
    assert_eq!(drain(&c.req), vec![Request::LoadPending]);
    let save = |n: u8, d: &str| rcad_cloud::cache::PendingSave {
        local_path: c.doc.clone(),
        document: "d1".into(),
        blob: format!("{n}").repeat(64),
        description: d.into(),
        milestone: None,
        base_version: 3,
        chained: n > 1,
        force: false,
        queued_at: 1_792_224_300,
        thumbnail: None,
    };
    c.ev.send(Event::PendingQueue {
        saves: vec![save(1, "thicker"), save(2, "")],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["queue_listed"], 2);
    assert!(c.h.query_by_label_contains("thicker").is_some());
    // Not asked again while the count is unchanged.
    assert!(drain(&c.req).is_empty());
    click(&mut c.h, "Discard queued version 1");
    assert_eq!(probe(&c)["dialog_kind"], "discard_pending");
    assert!(
        drain(&c.req).is_empty(),
        "nothing is discarded before confirming"
    );
    click(&mut c.h, "Confirm discard");
    assert_eq!(
        drain(&c.req),
        vec![Request::DiscardPending {
            index: 0,
            document: "d1".into(),
            blob: "1".repeat(64),
        }]
    );
    c.ev.send(Event::PendingQueue {
        saves: vec![save(2, "")],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["pending"], 1);
    assert!(drain(&c.req).is_empty());
}

#[test]
fn save_to_cloud_project_uploads_an_unlinked_document() {
    let mut c = signed_in([1400.0, 900.0]);
    // Not offered for a document that is not open, or without projects to pick.
    assert!(
        c.h.state()
            .command_enabled(rcad::shell::CmdId::SaveToCloud)
            .is_err()
    );
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    c.h.state_mut().request(Command::Open {
        path: c.doc.clone(),
        safe_mode: false,
    });
    settle(&mut c.h);
    file_menu(&mut c.h, "Save to Cloud Project…");
    assert_eq!(probe(&c)["dialog_kind"], "save_to_cloud");
    // The first editable project is chosen and its folders are loaded.
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadTree {
            project: "p1".into()
        }]
    );
    c.ev.send(Event::Tree(tree(
        vec![folder("f1", "Brackets", None)],
        vec![],
    )))
    .unwrap();
    settle(&mut c.h);
    assert_eq!(
        c.h.get_by_label("Document name").value().as_deref(),
        Some("Bracket")
    );
    click(&mut c.h, "Folder");
    click(&mut c.h, "Brackets");
    click(&mut c.h, "Save to Cloud");
    assert_eq!(
        drain(&c.req),
        vec![Request::UploadNew {
            local_path: c.doc.clone(),
            project: "p1".into(),
            folder: Some("f1".into()),
            name: "Bracket".into(),
            thumbnail: None,
        }]
    );
    // Once linked, Save Version is the way to save it.
    c.ev.send(Event::VersionSaved {
        link: link(&c.doc, 1),
        version: version(1, None),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(
        c.h.state()
            .command_enabled(rcad::shell::CmdId::SaveToCloud)
            .is_err()
    );
}

/// A small valid PNG (what a server sends as a thumbnail).
fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 120, 200, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

fn checks_head(reqs: &[Request]) -> bool {
    reqs.iter()
        .any(|r| matches!(r, Request::CheckHead { document } if document == "d1"))
}

#[test]
fn update_polls_are_never_sent_without_an_account() {
    let (svc, req, ev) = CloudService::detached();
    let mut h = harness([1400.0, 900.0]);
    let dir = tempfile::tempdir().unwrap();
    h.state_mut().attach_cloud_service(svc);
    assert_eq!(drain(&req), vec![Request::Refresh]);
    let doc = dir.path().join("Bracket.rcad.json");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bracket.rcad.json"),
        &doc,
    )
    .unwrap();
    // A linked working copy is open, but nobody is signed in.
    ev.send(Event::DocumentReady {
        path: doc.clone(),
        link: link(&doc, 3),
        read_only: false,
        from_cache: true,
    })
    .unwrap();
    settle(&mut h);
    click(&mut h, "Data panel");
    h.state_mut().cloud_expire_poll_timer();
    settle(&mut h);
    settle(&mut h);
    let sent = drain(&req);
    assert!(
        !sent
            .iter()
            .any(|r| matches!(r, Request::CheckHead { .. } | Request::ListProjects { .. })),
        "{sent:?}"
    );
}

#[test]
fn a_newer_version_on_the_server_offers_update() {
    let mut c = opened([1400.0, 900.0]);
    drain(&c.req);
    // Just opened: no poll until the interval passes.
    settle(&mut c.h);
    assert!(!checks_head(&drain(&c.req)));
    c.h.state_mut().cloud_expire_poll_timer();
    settle(&mut c.h);
    let sent = drain(&c.req);
    assert!(checks_head(&sent), "{sent:?}");
    // Polled once per interval, not every frame.
    settle(&mut c.h);
    assert!(!checks_head(&drain(&c.req)));

    // Same head: still synced.
    c.ev.send(Event::Head {
        document: "d1".into(),
        head_version: 3,
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · Synced");
    assert!(c.h.query_by_label("Update").is_none());

    c.ev.send(Event::Head {
        document: "d1".into(),
        head_version: 4,
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · v4 available");
    assert_eq!(probe(&c)["state"], "update_available");
    click(&mut c.h, "Update");
    assert_eq!(
        drain(&c.req),
        vec![Request::OpenDocument {
            document: "d1".into()
        }]
    );
    // The reopened working copy is at the head: synced again.
    c.ev.send(Event::DocumentReady {
        path: c.doc.clone(),
        link: link(&c.doc, 4),
        read_only: false,
        from_cache: false,
    })
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v4 · Synced");
}

#[test]
fn a_modified_copy_warns_instead_of_offering_update() {
    let mut c = opened([1400.0, 900.0]);
    edit(&mut c);
    drain(&c.req);
    c.ev.send(Event::Head {
        document: "d1".into(),
        head_version: 4,
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(chip(&c), "☁ Bracket v3 · Newer v4 on server");
    assert_eq!(probe(&c)["state"], "newer_on_server");
    assert!(c.h.query_by_label("Update").is_none());
    assert!(drain(&c.req).is_empty());
}

#[test]
fn opening_the_data_panel_polls_and_a_new_project_revision_reloads_the_tree() {
    let mut c = signed_in([1400.0, 900.0]);
    browsing(
        &mut c,
        proto::ProjectRole::Editor,
        tree(vec![], vec![summary(3)]),
    );
    drain(&c.req);
    settle(&mut c.h);
    assert!(drain(&c.req).is_empty(), "no poll right after loading");
    c.h.state_mut().cloud_expire_poll_timer();
    settle(&mut c.h);
    assert_eq!(
        drain(&c.req),
        vec![Request::ListProjects { org: "o1".into() }]
    );
    // Unchanged revision: nothing more.
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    settle(&mut c.h);
    assert!(drain(&c.req).is_empty());
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![proto::Project {
            revision: 2,
            ..project()
        }],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadTree {
            project: "p1".into()
        }]
    );
}

#[test]
fn thumbnails_load_once_and_show_in_rows_and_history() {
    let mut c = opened([1400.0, 900.0]);
    // Opening the Data panel checks the open document's head at once.
    click(&mut c.h, "Data panel");
    assert!(checks_head(&drain(&c.req)));
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Project Parts");
    drain(&c.req);
    // A document row whose head has a thumbnail asks for it once.
    c.ev.send(Event::Tree(tree(
        vec![],
        vec![proto::DocumentSummary {
            has_thumbnail: true,
            ..summary(3)
        }],
    )))
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    settle(&mut c.h);
    let sent = drain(&c.req);
    let asks = |reqs: &[Request], v: u64| {
        reqs.iter()
            .filter(|r| {
                matches!(r, Request::LoadThumbnail { document, version }
                    if document == "d1" && *version == v)
            })
            .count()
    };
    assert_eq!(asks(&sent, 3), 1, "{sent:?}");
    c.ev.send(Event::Thumbnail {
        document: "d1".into(),
        version: 3,
        png: Some(tiny_png()),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(c.h.state().cloud_thumbnail_loaded("d1", 3));
    assert!(c.h.query_by_label("Thumbnail of Bracket").is_some());

    // Version history: v2 has a thumbnail, v1 none.
    file_menu(&mut c.h, "Version History");
    drain(&c.req);
    let with_thumb = proto::Version {
        thumbnail: Some("b".repeat(64)),
        ..version(2, None)
    };
    c.ev.send(Event::Detail(proto::DocumentDetail {
        document: summary(3),
        versions: vec![version(3, None), with_thumb, version(1, None)],
        my_role: proto::ProjectRole::Editor,
    }))
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    let sent = drain(&c.req);
    assert_eq!(asks(&sent, 2), 1, "{sent:?}");
    assert_eq!(asks(&sent, 1), 0, "{sent:?}");
    assert!(c.h.query_by_label("Thumbnail of v2").is_none());
    // A bad PNG gives no texture and is not asked for again.
    c.ev.send(Event::Thumbnail {
        document: "d1".into(),
        version: 2,
        png: Some(b"\x89PNG\r\n\x1a\nbroken".to_vec()),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(!c.h.state().cloud_thumbnail_loaded("d1", 2));
    assert_eq!(asks(&drain(&c.req), 2), 0);
    // A fresh history reload of another document version with a good PNG.
    c.ev.send(Event::Thumbnail {
        document: "d1".into(),
        version: 2,
        png: Some(tiny_png()),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(c.h.state().cloud_thumbnail_loaded("d1", 2));
    assert!(c.h.query_by_label("Thumbnail of v2").is_some());
}

#[test]
fn a_version_save_without_a_gpu_view_goes_out_at_once_without_a_thumbnail() {
    let mut c = opened([1400.0, 900.0]);
    c.h.state_mut().set_cloud_thumbnail_capture(true);
    drain(&c.req);
    file_menu(&mut c.h, "Save Version to Cloud…");
    type_into(&mut c.h, "Version description", "with a picture");
    click(&mut c.h, "Save Version");
    // The thumbnail is rendered offscreen in the same frame; the test harness has
    // no GPU view, so the request goes out immediately, without one.
    match drain(&c.req).as_slice() {
        [
            Request::SaveVersion {
                description,
                thumbnail: None,
                ..
            },
        ] => assert_eq!(description, "with a picture"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn comments_are_counted_for_the_probe() {
    let mut c = opened([1400.0, 900.0]);
    if !c.h.state().cloud_panel_open() {
        click(&mut c.h, "Data panel");
    }
    drain(&c.req);
    click(&mut c.h, "Comments section");
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadComments {
            document: "d1".into()
        }]
    );
    let comment = |id: &str, resolved: bool| proto::Comment {
        id: id.into(),
        document: "d1".into(),
        version: 3,
        author: "bob".into(),
        body: format!("note {id}"),
        created_at: 1_792_224_300,
        resolved,
        parent: None,
        anchor: None,
    };
    c.ev.send(Event::Comments {
        document: "d1".into(),
        comments: vec![comment("c1", false), comment("c2", true)],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["comments_listed"], 2);
    assert_eq!(probe(&c)["comments_resolved"], 1);
    click(&mut c.h, "Resolve comment 1");
    assert_eq!(
        drain(&c.req),
        vec![Request::ResolveComment {
            document: "d1".into(),
            comment: "c1".into(),
            resolved: true,
        }]
    );
}

fn share_link(id: &str, version: Option<u64>) -> proto::ShareLink {
    proto::ShareLink {
        id: id.into(),
        document: "d1".into(),
        version,
        created_by: "alice".into(),
        created_at: 1_792_224_300,
        expires_at: version.map(|_| 1_792_224_300 + 7 * 86_400),
        allow_download: version.is_none(),
    }
}

#[test]
fn share_dialog_lists_links_and_revokes_after_confirmation() {
    let mut c = opened([1400.0, 900.0]);
    file_menu(&mut c.h, "Share Link…");
    drain(&c.req);
    assert_eq!(probe(&c)["shares_listed"], 0);
    c.ev.send(Event::Shares {
        document: "d1".into(),
        shares: vec![share_link("s1", Some(2)), share_link("s2", None)],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["shares_listed"], 2);
    assert!(c.h.query_by_label_contains("v2 · expires 2026-").is_some());
    assert!(
        c.h.query_by_label_contains("latest version · never expires · download allowed · by alice")
            .is_some()
    );
    // Revoke asks first; Keep backs out without a request.
    click(&mut c.h, "Revoke share link 2");
    assert!(drain(&c.req).is_empty());
    click(&mut c.h, "Keep link");
    assert!(c.h.query_by_label("Confirm revoke").is_none());
    click(&mut c.h, "Revoke share link 2");
    click(&mut c.h, "Confirm revoke");
    assert_eq!(
        drain(&c.req),
        vec![Request::RevokeShare {
            document: "d1".into(),
            share: "s2".into(),
        }]
    );
    c.ev.send(Event::Shares {
        document: "d1".into(),
        shares: vec![share_link("s1", Some(2))],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["shares_listed"], 1);
    assert!(c.h.query_by_label("Confirm revoke").is_none());
    // A failed revoke is shown (not a background request).
    c.ev.send(Event::Error {
        request: "revoke_share",
        code: Some(proto::ErrorCode::Forbidden),
        message: "only the link's creator or a project admin can revoke it".into(),
    })
    .unwrap();
    settle(&mut c.h);
    assert!(
        c.h.state()
            .diagnostics()
            .iter()
            .any(|d| d.error && d.text.contains("only the link's creator"))
    );
}

fn entry(summary: &str, document: Option<&str>) -> proto::ActivityEntry {
    proto::ActivityEntry {
        at: 1_792_224_300,
        actor: "bob".into(),
        action: "version.create".into(),
        summary: summary.into(),
        project: Some("p1".into()),
        document: document.map(String::from),
    }
}

#[test]
fn activity_section_lists_the_project_and_opens_documents() {
    let mut c = signed_in([1400.0, 900.0]);
    click(&mut c.h, "Data panel");
    c.ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project()],
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Project Parts");
    drain(&c.req);
    click(&mut c.h, "Activity section");
    assert_eq!(
        drain(&c.req),
        vec![Request::LoadActivity {
            project: "p1".into()
        }]
    );
    c.ev.send(Event::Activity {
        project: "p1".into(),
        entries: vec![
            entry("saved version 4 of Bracket", Some("d1")),
            entry("added carol to Parts", None),
        ],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["activity_listed"], 2);
    assert!(
        c.h.query_by_label_contains("added carol to Parts")
            .is_some()
    );
    click(&mut c.h, "Activity entry 1");
    assert_eq!(
        drain(&c.req),
        vec![Request::OpenDocument {
            document: "d1".into()
        }]
    );
    // Refreshed with the poll while the section is shown.
    c.h.state_mut().cloud_expire_poll_timer();
    settle(&mut c.h);
    assert!(drain(&c.req).contains(&Request::LoadActivity {
        project: "p1".into()
    }));
    // Not when another section is shown.
    click(&mut c.h, "Browse section");
    drain(&c.req);
    c.h.state_mut().cloud_expire_poll_timer();
    settle(&mut c.h);
    assert!(
        !drain(&c.req)
            .iter()
            .any(|r| matches!(r, Request::LoadActivity { .. }))
    );
}

#[test]
fn organization_window_shows_activity_for_admins() {
    let mut c = signed_in([1400.0, 900.0]);
    let mut owner = me();
    owner.orgs[0].my_role = proto::OrgRole::Admin;
    c.ev.send(Event::SignedIn {
        server_url: "http://127.0.0.1:18640".into(),
        server_name: "Home lab".into(),
        me: owner,
    })
    .unwrap();
    settle(&mut c.h);
    click(&mut c.h, "Data panel");
    drain(&c.req);
    click(&mut c.h, "Organization…");
    assert!(drain(&c.req).contains(&Request::LoadOrgActivity { org: "o1".into() }));
    c.ev.send(Event::OrgActivity {
        org: "o1".into(),
        entries: vec![entry("saved version 2 of Hinge", Some("d9"))],
    })
    .unwrap();
    settle(&mut c.h);
    assert_eq!(probe(&c)["org_activity_listed"], 1);
    click(&mut c.h, "Organization activity entry 1");
    assert_eq!(
        drain(&c.req),
        vec![Request::OpenDocument {
            document: "d9".into()
        }]
    );
    assert!(c.h.state().cloud_dialog().is_none(), "the window closes");
}

/// Save the open document locally (as Ctrl+S would) and return the requests sent.
fn save_locally(c: &mut Cloud, path: &Path) -> Vec<Request> {
    drain(&c.req);
    c.h.state_mut().request(Command::Save {
        path: path.to_path_buf(),
    });
    settle(&mut c.h);
    settle(&mut c.h);
    drain(&c.req)
}

fn version_saves(reqs: &[Request]) -> Vec<&Request> {
    reqs.iter()
        .filter(|r| matches!(r, Request::SaveVersion { .. }))
        .collect()
}

#[test]
fn version_on_save_is_off_by_default_and_uploads_linked_saves_when_on() {
    let mut c = opened([1400.0, 900.0]);
    let doc = c.doc.clone();
    edit(&mut c);
    assert!(
        version_saves(&save_locally(&mut c, &doc)).is_empty(),
        "off by default"
    );
    click(&mut c.h, "Preferences");
    click(&mut c.h, "Pref cloud version on save");
    h_close_prefs(&mut c);
    edit(&mut c);
    let sent = save_locally(&mut c, &doc);
    match version_saves(&sent).as_slice() {
        [
            Request::SaveVersion {
                local_path,
                description,
                milestone: None,
                force: false,
                ..
            },
        ] => {
            assert_eq!(local_path, &doc);
            assert_eq!(description, "Saved from R-CAD");
        }
        other => panic!("{other:?}"),
    }
    // A version save that saves locally first sends one version, not two.
    edit(&mut c);
    file_menu(&mut c.h, "Save Version to Cloud…");
    type_into(&mut c.h, "Version description", "by hand");
    drain(&c.req);
    click(&mut c.h, "Save Version");
    settle(&mut c.h);
    settle(&mut c.h);
    let sent = drain(&c.req);
    match version_saves(&sent).as_slice() {
        [Request::SaveVersion { description, .. }] => assert_eq!(description, "by hand"),
        other => panic!("{other:?}"),
    }
}

fn h_close_prefs(c: &mut Cloud) {
    c.h.key_press(Key::Escape);
    settle(&mut c.h);
}

#[test]
fn version_on_save_never_uploads_unlinked_or_read_only_documents() {
    let mut c = signed_in([1400.0, 900.0]);
    click(&mut c.h, "Preferences");
    click(&mut c.h, "Pref cloud version on save");
    h_close_prefs(&mut c);
    // An ordinary local document.
    let doc = c.doc.clone();
    c.h.state_mut().request(Command::Open {
        path: doc.clone(),
        safe_mode: false,
    });
    settle(&mut c.h);
    settle(&mut c.h);
    edit(&mut c);
    assert!(version_saves(&save_locally(&mut c, &doc)).is_empty());
    // A read-only copy of an older version.
    c.ev.send(Event::DocumentReady {
        path: doc.clone(),
        link: link(&doc, 2),
        read_only: true,
        from_cache: false,
    })
    .unwrap();
    settle(&mut c.h);
    settle(&mut c.h);
    edit(&mut c);
    assert!(version_saves(&save_locally(&mut c, &doc)).is_empty());
}
