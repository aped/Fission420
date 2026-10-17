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
        drain(&req),
        vec![Request::SignIn {
            server_url: "http://127.0.0.1:18640".into(),
            username: "alice".into(),
            password: "alice-pass-123".into(),
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
        drain(&req),
        vec![Request::ListProjects { org: "o1".into() }]
    );
    click(&mut h, "Sign out");
    assert_eq!(drain(&req), vec![Request::SignOut]);
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
        vec![Request::LoadDetail {
            document: "d1".into()
        }]
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
            Request::LoadUsage { org: "o1".into() }
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
