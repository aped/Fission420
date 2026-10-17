//! `rcad-cli cloud` exit codes and output against the in-memory fake server (127.0.0.1 only).
#[path = "../../rcad-cloud/tests/fake_server/mod.rs"]
mod fake_server;

use fake_server::{FakeServer, PASSWORD, PROJECT, USER};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cli(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rcad-cli"));
        c.arg("cloud")
            .args(args)
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env_remove("RCAD_PASSWORD")
            .current_dir(self.dir.path());
        match stdin {
            Some(text) => {
                use std::io::Write;
                c.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let mut child = c.spawn().unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(text.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            }
            None => c.stdin(std::process::Stdio::null()).output().unwrap(),
        }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn login(env: &Env, url: &str) {
    let o = env.cli(&["login", url, USER], Some(&format!("{PASSWORD}\n")));
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("signed in"), "{}", text(&o));
}

/// `push --json`: the new document's id.
fn push(env: &Env, file: &Path) -> String {
    let o = env.cli(
        &[
            "--json",
            "push",
            file.to_str().unwrap(),
            "--project",
            PROJECT,
        ],
        None,
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["event"], "version_saved");
    assert_eq!(v["version"]["number"], 1);
    v["link"]["document"].as_str().unwrap().to_string()
}

#[test]
fn usage_and_signed_out_errors() {
    let env = Env::new();
    assert_eq!(code(&env.cli(&[], None)), 64);
    assert_eq!(code(&env.cli(&["frobnicate"], None)), 64);
    assert_eq!(code(&env.cli(&["save"], None)), 64);
    assert_eq!(code(&env.cli(&["help"], None)), 0);
    let o = env.cli(&["whoami"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("not signed in"), "{}", text(&o));
    assert!(
        !env.dir.path().join("config").exists(),
        "nothing written when signed out"
    );
}

#[test]
fn login_push_save_conflict_force_and_lock() {
    let server = FakeServer::start();
    let env = Env::new();
    // Wrong password from stdin: error, no account.
    let o = env.cli(&["login", &server.url, USER], Some("nope\n"));
    assert_eq!(code(&o), 1, "{}", text(&o));
    login(&env, &server.url);
    let o = env.cli(&["whoami"], None);
    assert_eq!(code(&o), 0);
    assert!(text(&o).contains(USER), "{}", text(&o));
    assert!(
        !text(&o).contains("rcs_"),
        "token never printed: {}",
        text(&o)
    );
    let o = env.cli(&["orgs"], None);
    assert!(text(&o).contains(fake_server::ORG), "{}", text(&o));

    let file = env.file("Bracket.rcad.json", b"one");
    let doc = push(&env, &file);
    std::fs::write(&file, b"two").unwrap();
    let o = env.cli(
        &[
            "save",
            "Bracket.rcad.json",
            "-m",
            "second",
            "--milestone",
            "M1",
        ],
        None,
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("version 2"), "{}", text(&o));

    server.state.lock().unwrap().save_as(&doc, "bob", b"bob");
    std::fs::write(&file, b"three").unwrap();
    let o = env.cli(&["save", "Bracket.rcad.json"], None);
    assert_eq!(code(&o), 3, "{}", text(&o));
    let o = env.cli(&["save", "Bracket.rcad.json", "--force"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));

    server.state.lock().unwrap().doc_mut(&doc).unwrap().lock = Some(rcad_cloud::proto::Lock {
        holder: "bob".into(),
        since: 1,
        expires_at: 2,
    });
    std::fs::write(&file, b"four").unwrap();
    let o = env.cli(&["save", "Bracket.rcad.json"], None);
    assert_eq!(code(&o), 4, "{}", text(&o));
    assert!(text(&o).contains("bob"), "{}", text(&o));

    let o = env.cli(&["versions", &doc], None);
    assert_eq!(code(&o), 0);
    assert!(text(&o).contains("* M1"), "{}", text(&o));
    let o = env.cli(&["tree", PROJECT], None);
    assert!(text(&o).contains("Bracket"), "{}", text(&o));
    let o = env.cli(&["share", &doc, "--days", "3", "--download"], None);
    assert_eq!(code(&o), 0);
    assert!(text(&o).contains("/s/"), "{}", text(&o));

    let out = env.dir.path().join("copy.rcad.json");
    let o = env.cli(
        &[
            "pull",
            &doc,
            "--version",
            "1",
            "--out",
            out.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert_eq!(std::fs::read(&out).unwrap(), b"one");

    let o = env.cli(&["logout"], None);
    assert_eq!(code(&o), 0);
    assert_eq!(code(&env.cli(&["whoami"], None)), 1);
}

#[test]
fn offline_save_queues_then_sync_sends() {
    let server = FakeServer::start();
    let port = server.port;
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Shaft.rcad.json", b"one");
    let doc = push(&env, &file);
    let state = server.stop();

    std::fs::write(&file, b"two").unwrap();
    let o = env.cli(&["save", "Shaft.rcad.json"], None);
    assert_eq!(code(&o), 5, "{}", text(&o));
    assert_eq!(code(&env.cli(&["sync"], None)), 5);
    // Read-only commands fail plainly while offline; pull falls back to the cached copy.
    assert_eq!(code(&env.cli(&["tree", PROJECT], None)), 1);
    let o = env.cli(&["pull", &doc], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("cached copy"), "{}", text(&o));

    let server = FakeServer::start_on(port, state);
    let o = env.cli(&["sync"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("pending versions: 0"), "{}", text(&o));
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .doc_mut(&doc)
            .unwrap()
            .versions
            .len(),
        2
    );
}

#[test]
fn admin_command_usage() {
    let env = Env::new();
    for args in [
        &["project"][..],
        &["project", "delete", "o1", "X"],
        &["project", "create", "o1"],
        &["members"],
        &["member", "add", "p1", "bob"],
        &["member", "add", "p1", "bob", "owner"],
        &["member", "remove", "p1"],
        &["member", "promote", "p1", "bob", "editor"],
        &["mkdir", "p1"],
        &["mkdir", "p1", "X", "--parent"],
        &["trash"],
        &["restore", "a", "b"],
    ] {
        let o = env.cli(args, None);
        assert_eq!(code(&o), 64, "{args:?}: {}", text(&o));
    }
    // Signed out: a plain error, nothing sent.
    let o = env.cli(&["members", "p1"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("not signed in"), "{}", text(&o));
}

#[test]
fn project_create_and_mkdir() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let o = env.cli(&["project", "create", fake_server::ORG, "Fixtures"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("Fixtures\tadmin"), "{}", text(&o));
    let o = env.cli(&["projects", fake_server::ORG], None);
    assert!(text(&o).contains("Fixtures"), "{}", text(&o));

    let o = env.cli(&["--json", "mkdir", PROJECT, "Brackets"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).trim()).unwrap();
    assert_eq!(v["event"], "folder_created");
    let parent = v["folder"]["id"].as_str().unwrap().to_string();
    let o = env.cli(&["mkdir", PROJECT, "M6", "--parent", &parent], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let st = server.state.lock().unwrap();
    let m6 = st.folders.iter().find(|f| f.name == "M6").unwrap();
    assert_eq!(m6.parent.as_deref(), Some(parent.as_str()));
    assert_eq!(m6.project, PROJECT);
}

#[test]
fn members_and_trash_reach_their_routes() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let o = env.cli(&["member", "add", PROJECT, "bob", "Editor"], None);
    assert!(
        server
            .log()
            .iter()
            .any(|l| l == &format!("PUT /api/v1/projects/{PROJECT}/members")),
        "{:?}",
        server.log()
    );
    if text(&o).contains("no route") {
        // A fake server without the member and trash routes: only the requests are checked.
        let file = env.file("Plate.rcad.json", b"one");
        let doc = push(&env, &file);
        let _ = env.cli(&["trash", &doc], None);
        let _ = env.cli(&["restore", &doc], None);
        let _ = env.cli(&["members", PROJECT], None);
        let _ = env.cli(&["member", "remove", PROJECT, "bob"], None);
        let log = server.log();
        for want in [
            format!("DELETE /api/v1/documents/{doc}"),
            format!("POST /api/v1/documents/{doc}/restore"),
            format!("GET /api/v1/projects/{PROJECT}/members"),
            format!("DELETE /api/v1/projects/{PROJECT}/members/bob"),
        ] {
            assert!(log.contains(&want), "{want} not in {log:?}");
        }
        return;
    }
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("bob is editor"), "{}", text(&o));
    let o = env.cli(&["members", PROJECT], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("bob\teditor"), "{}", text(&o));
    let o = env.cli(&["member", "set", PROJECT, "bob", "viewer"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    // Not a member of the organization: 404 with the explanation.
    let o = env.cli(&["member", "add", PROJECT, "carol", "editor"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains("add them to the organization first"),
        "{}",
        text(&o)
    );
    let o = env.cli(&["member", "remove", PROJECT, "bob"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let o = env.cli(&["--json", "members", PROJECT], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).trim()).unwrap();
    assert_eq!(v["members"].as_array().map(Vec::len), Some(0));

    let file = env.file("Plate.rcad.json", b"one");
    let doc = push(&env, &file);
    let o = env.cli(&["trash", &doc], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let o = env.cli(&["tree", PROJECT], None);
    assert!(text(&o).contains("trash:"), "{}", text(&o));
    let o = env.cli(&["restore", &doc], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("restored Plate"), "{}", text(&o));
    let o = env.cli(&["restore", &doc], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
}
