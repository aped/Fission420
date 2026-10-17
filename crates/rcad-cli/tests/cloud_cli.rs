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
        let mut all = vec!["cloud"];
        all.extend_from_slice(args);
        self.raw(&all, stdin)
    }

    /// `rcad-cli ARGS` (no `cloud` added) with this environment's directories.
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rcad-cli"));
        c.args(args)
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env_remove("RCAD_PASSWORD")
            .current_dir(self.dir.path());
        c
    }

    fn raw(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut c = self.command(args);
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
    // whoami offline still names the account in use.
    let o = env.cli(&["whoami"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains(&format!(
            "{USER} on Test lab <http://127.0.0.1:{port}> (server unreachable)"
        )),
        "{}",
        text(&o)
    );
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
fn queue_lists_and_discard_drops_a_queued_version() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Pin.rcad.json", b"one");
    let doc = push(&env, &file);
    drop(server.stop());
    std::fs::write(&file, b"two").unwrap();
    let o = env.cli(&["save", "Pin.rcad.json", "-m", "longer"], None);
    assert_eq!(code(&o), 5, "{}", text(&o));
    // Listing and discarding work offline: the queue is local.
    let o = env.cli(&["queue"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(
        out.starts_with(&format!("0\t{doc}\t")) && out.contains("longer"),
        "{out}"
    );
    let o = env.cli(&["discard", "3"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert_eq!(code(&env.cli(&["discard", "x"], None)), 64);
    // Not a terminal and no --yes: refused as a usage error, nothing dropped.
    let o = env.cli(&["discard", "0"], None);
    assert_eq!(code(&o), 64, "{}", text(&o));
    assert!(text(&o).contains("--yes"), "{}", text(&o));
    assert!(
        String::from_utf8_lossy(&env.cli(&["queue"], None).stdout).contains("longer"),
        "still queued"
    );
    let o = env.cli(&["discard", "0", "--yes", "--json"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["event"], "discarded");
    assert_eq!(lines[0]["document"], doc.as_str());
    assert_eq!(lines[0]["description"], "longer");
    assert_eq!(lines[1]["event"], "pending_queue");
    assert_eq!(lines[1]["saves"].as_array().map(Vec::len), Some(0));
    assert_eq!(std::fs::read(&file).unwrap(), b"two");
}

#[test]
fn discard_with_yes_says_what_it_dropped() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Cam.rcad.json", b"one");
    let doc = push(&env, &file);
    drop(server.stop());
    std::fs::write(&file, b"two").unwrap();
    assert_eq!(
        code(&env.cli(&["save", "Cam.rcad.json", "-m", "lobe"], None)),
        5
    );
    let o = env.cli(&["discard", "-y", "0"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(
        out.starts_with(&format!("discarded 0: {doc} queued ")),
        "{out}"
    );
    assert!(out.contains("lobe"), "{out}");
    assert!(
        env.cli(&["queue"], None).stdout.is_empty(),
        "queue is empty"
    );
}

#[test]
fn closed_stdout_exits_quietly_without_a_panic() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Long.rcad.json", b"v1");
    let doc = push(&env, &file);
    {
        // More output than a pipe buffers, so the write fails whatever the timing.
        let mut st = server.state.lock().unwrap();
        for i in 0..3000 {
            st.save_as(&doc, "bob", format!("v{i}").as_bytes());
        }
    }
    let mut child = env
        .command(&["cloud", "versions", &doc])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // `| head -0`: the reader goes away before reading anything.
    drop(child.stdout.take());
    let o = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(code(&o), 141, "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.is_empty(), "nothing on stderr: {err}");
}

#[test]
fn push_prints_the_new_document_id() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    env.file("Hinge.rcad.json", b"one");
    let o = env.cli(&["push", "Hinge.rcad.json", "--project", PROJECT], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let id = out
        .lines()
        .find_map(|l| l.strip_prefix("document "))
        .unwrap_or_else(|| panic!("no document line: {out}"))
        .to_string();
    let st = server.state.lock().unwrap();
    assert_eq!(st.docs.len(), 1);
    assert_eq!(st.docs[0].summary.id, id);
}

#[test]
fn json_and_account_options_go_anywhere() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Plate.rcad.json", b"one");
    let doc = push(&env, &file);
    let first_json = |o: &Output| -> serde_json::Value {
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        serde_json::from_str(out.lines().next().unwrap_or(""))
            .unwrap_or_else(|e| panic!("{e}: {out}"))
    };
    // Before `cloud`, after `cloud`, after the command and after its arguments.
    for args in [
        vec!["--json", "cloud", "versions", &doc],
        vec!["cloud", "--json", "versions", &doc],
        vec!["cloud", "versions", "--json", &doc],
        vec!["cloud", "versions", &doc, "--json"],
        vec![
            "--account",
            &server.url,
            "--json",
            "cloud",
            "versions",
            &doc,
        ],
        vec![
            "cloud",
            "versions",
            &doc,
            "--json",
            "--account",
            &server.url,
        ],
    ] {
        let o = env.raw(&args, None);
        assert_eq!(code(&o), 0, "{args:?}: {}", text(&o));
        assert_eq!(first_json(&o)["event"], "detail", "{args:?}");
    }
    let acct = format!("--account={}#{USER}", server.url);
    let o = env.cli(&["versions", &doc, &acct, "--json"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert_eq!(code(&env.raw(&["--json"], None)), 64);
    assert_eq!(code(&env.raw(&["--json", "run"], None)), 64);
    assert_eq!(code(&env.cli(&["versions", &doc, "--account"], None)), 64);
    assert_eq!(
        code(&env.cli(&["versions", &doc, "--account=#ada"], None)),
        64
    );
}

#[test]
fn accounts_use_and_one_command_account_override() {
    let a = FakeServer::start();
    let b = FakeServer::start();
    let env = Env::new();
    login(&env, &a.url);
    login(&env, &b.url);
    // The last sign-in is active.
    let o = env.cli(&["accounts"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(
        out.contains(&format!("  {}\t{USER}\tTest lab", a.url)),
        "{out}"
    );
    assert!(
        out.contains(&format!("* {}\t{USER}\tTest lab", b.url)),
        "{out}"
    );
    assert!(!out.contains("rcs_"), "no token: {out}");

    // --account: this command only; the file's active account is unchanged.
    let before = a.log().len();
    let o = env.cli(&["--account", &a.url, "whoami"], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains(&a.url), "{}", text(&o));
    assert!(a.log().len() > before, "request went to the named server");
    let o = env.cli(&["whoami"], None);
    assert!(text(&o).contains(&b.url), "{}", text(&o));
    let o = env.cli(&["--account", "https://nowhere.example", "whoami"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("no account on"), "{}", text(&o));
    assert_eq!(code(&env.cli(&["--account", &a.url, "accounts"], None)), 64);
    assert_eq!(
        code(&env.cli(&["--account", &a.url, "login", &a.url, USER], None)),
        64
    );

    // use: switches the active account (the trailing slash is normalized away).
    let o = env.cli(&["use", &format!("{}/", a.url), USER], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(
        text(&o).contains(&format!("using {USER} on Test lab <{}>", a.url)),
        "{}",
        text(&o)
    );
    let o = env.cli(&["--json", "accounts"], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).trim()).unwrap();
    let list = v["accounts"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["server_url"], a.url.as_str());
    assert_eq!(list[0]["active"], true);
    assert_eq!(list[1]["active"], false);
    assert!(list[0].get("token").is_none());
    let o = env.cli(&["use", &a.url, "nobody"], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert_eq!(code(&env.cli(&["use"], None)), 64);

    // logout with --account signs out of that account; the active one stays.
    let o = env.cli(&["logout", "--account", &b.url], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&env.cli(&["accounts"], None).stdout).to_string();
    assert_eq!(out.trim_end(), format!("* {}\t{USER}\tTest lab", a.url));
}

#[test]
fn shares_lists_and_unshare_revokes() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("Gear.rcad.json", b"one");
    let doc = push(&env, &file);
    assert_eq!(
        code(&env.cli(&["share", &doc, "--days", "7", "--download"], None)),
        0
    );
    assert_eq!(code(&env.cli(&["share", &doc, "--version", "1"], None)), 0);
    let o = env.cli(&["shares", &doc], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].contains("\tlatest\texpires 1970-01-08 00:00 UTC\tdownload\tby ada"),
        "{out}"
    );
    assert!(
        lines[1].contains("\tv1\tno expiry\tview only\tby ada"),
        "{out}"
    );
    let first = lines[0].split('\t').next().unwrap().to_string();

    let o = env.cli(&["unshare", &first], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(
        text(&o).contains(&format!("revoked share link {first}")),
        "{}",
        text(&o)
    );
    assert!(
        server
            .log()
            .iter()
            .any(|l| l.ends_with(&format!("/shares/{first}")))
    );
    let o = env.cli(&["--json", "shares", &doc], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).trim()).unwrap();
    assert_eq!(v["event"], "shares");
    assert_eq!(v["shares"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["shares"][0]["version"], 1);
    // Revoking again: not found, with a hint.
    let o = env.cli(&["unshare", &first], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("rcad-cli cloud shares"), "{}", text(&o));
    assert_eq!(code(&env.cli(&["shares"], None)), 64);
    assert_eq!(code(&env.cli(&["unshare", "a", "b"], None)), 64);
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

#[test]
fn hostile_server_text_is_escaped_in_text_output() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("part.rcad.json", b"v1");
    let doc = push(&env, &file);
    let evil_name = "Bracket\u{1b}]52;c;ZXZpbA==\u{7}\u{202E}fdp";
    let evil_text = "ok\u{1b}[2J\nsaved: forged line\u{9b}31m";
    {
        let mut st = server.state.lock().unwrap();
        let d = st.doc_mut(&doc).unwrap();
        d.summary.name = evil_name.into();
        d.versions[0].description = evil_text.into();
        d.versions[0].author = "mallory\u{1b}[8m".into();
        d.lock = Some(rcad_cloud::proto::Lock {
            holder: "eve\u{1b}]0;pwned\u{7}".into(),
            since: 1,
            expires_at: 2,
        });
    }
    let outputs = [
        env.cli(&["tree", PROJECT], None),
        env.cli(&["versions", &doc], None),
        env.cli(&["save", file.to_str().unwrap()], None),
    ];
    for o in &outputs {
        let all = [o.stdout.as_slice(), o.stderr.as_slice()].concat();
        let s = String::from_utf8_lossy(&all);
        assert!(
            !s.chars().any(|c| (c.is_control() && c != '\n' && c != '\t')
                || ('\u{202A}'..='\u{202E}').contains(&c)),
            "{s:?}"
        );
        assert!(s.contains("\\u{1b}"), "{s:?}");
    }
    let tree = text(&outputs[0]);
    assert!(tree.contains("\\u{202e}fdp"), "{tree}");
    let versions = text(&outputs[1]);
    assert!(
        versions.contains("\\u{a}saved: forged line"),
        "the newline is escaped, not a new line: {versions}"
    );
    assert_eq!(code(&outputs[2]), 4, "{}", text(&outputs[2]));
    // JSON output carries the exact strings (escaped by the JSON encoder).
    let o = env.cli(&["--json", "versions", &doc], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(v["detail"]["document"]["name"], evil_name);
    assert_eq!(v["detail"]["versions"][0]["description"], evil_text);
}

#[test]
fn plain_http_login_to_a_public_host_needs_the_flag() {
    let env = Env::new();
    // Refused before the password is read (stdin is closed) or any request is made.
    let o = env.cli(&["login", "http://cad.example", USER], None);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("--allow-insecure-http"), "{}", text(&o));
    assert!(text(&o).contains("unencrypted"), "{}", text(&o));
    assert!(!env.dir.path().join("config").exists());
    // With the flag: a warning, then the request goes out (`.invalid` never resolves).
    let o = env.cli(
        &[
            "login",
            "http://rcad.invalid",
            USER,
            "--allow-insecure-http",
        ],
        Some("pw\n"),
    );
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("warning:"), "{}", text(&o));
    assert!(text(&o).contains("unreachable"), "{}", text(&o));
    // Loopback needs no flag.
    let server = FakeServer::start();
    login(&env, &server.url);
}

#[test]
fn concurrent_cli_processes_queue_every_save() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let files: Vec<PathBuf> = (0..4)
        .map(|i| {
            let f = env.file(&format!("p{i}.rcad.json"), format!("v1-{i}").as_bytes());
            push(&env, &f);
            f
        })
        .collect();
    drop(server.stop());
    // Twelve `save` processes at once (three per file), all offline: each queues.
    let rounds = 3;
    let children: Vec<_> = (0..rounds)
        .flat_map(|r| files.iter().map(move |f| (r, f.clone())))
        .map(|(r, f)| {
            std::fs::write(&f, format!("round {r} {}", f.display())).unwrap();
            Command::new(env!("CARGO_BIN_EXE_rcad-cli"))
                .args(["cloud", "save", f.to_str().unwrap(), "-m"])
                .arg(format!("round {r}"))
                .env("HOME", env.dir.path())
                .env("XDG_CONFIG_HOME", env.dir.path().join("config"))
                .env("XDG_CACHE_HOME", env.dir.path().join("cache"))
                .env_remove("RCAD_PASSWORD")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let o = child.wait_with_output().unwrap();
        assert_eq!(code(&o), 5, "{}", text(&o));
    }
    let o = env.cli(&["--json", "queue"], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(
        v["saves"].as_array().unwrap().len(),
        files.len() * rounds,
        "{v}"
    );
}

#[test]
fn save_and_push_take_an_optional_thumbnail() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let mut png = rcad_cloud::client::PNG_SIGNATURE.to_vec();
    png.extend_from_slice(b"thumb");
    let thumb = env.file("t.png", &png);
    let not_png = env.file("t.gif", b"GIF89a");
    let file = env.file("part.rcad.json", b"v1");
    let o = env.cli(
        &[
            "--json",
            "push",
            file.to_str().unwrap(),
            "--project",
            PROJECT,
            "--thumbnail",
            thumb.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(v["version"]["thumbnail"], rcad_cloud::blob_hash(&png));
    std::fs::write(&file, b"v2").unwrap();
    let posts = |s: &FakeServer| s.log().iter().filter(|l| l.ends_with("/versions")).count();
    let before = posts(&server);
    let o = env.cli(
        &[
            "save",
            file.to_str().unwrap(),
            "--thumbnail",
            not_png.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("not a PNG"), "{}", text(&o));
    assert_eq!(posts(&server), before, "nothing sent");
    let o = env.cli(
        &[
            "save",
            file.to_str().unwrap(),
            "--thumbnail",
            thumb.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
}

#[test]
fn activity_lists_the_project_log() {
    let server = FakeServer::start();
    let env = Env::new();
    login(&env, &server.url);
    let file = env.file("arm.rcad.json", b"v1");
    push(&env, &file);
    let o = env.cli(&["activity", PROJECT], None);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(
        text(&o).contains("version.create\tsaved version 1 of arm"),
        "{}",
        text(&o)
    );
    let o = env.cli(&["--json", "activity", PROJECT], None);
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(v["event"], "activity");
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(code(&env.cli(&["activity"], None)), 64);
}
