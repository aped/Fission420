//! `rcad-cli cloud ...`: the optional sync server from the command line.
//!
//! Uses the same account file and cache as the app (`rcad_cloud::config::default_path`,
//! `rcad_cloud::cache::default_root`), so a CLI sign-in works for the app too. Every command
//! runs one or more `rcad_cloud::service::Request`s synchronously and prints the resulting
//! events, as lines of text or (with `--json`) one JSON object per event.
//!
//! Administration commands (projects, members, folders, trash) call the client library
//! directly and print the server's answer.
//!
//! Exit codes: 0 ok, 1 error, 3 version conflict, 4 locked by another user, 5 server
//! unreachable and the version was queued (`rcad-cli cloud sync` sends it later), 64 usage.
use rcad_cloud::CloudError;
use rcad_cloud::cache::Link;
use rcad_cloud::client::Client;
use rcad_cloud::proto;
use rcad_cloud::service::{Event, Request, Worker};
use serde_json::json;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: rcad-cli cloud [--json] COMMAND
  login SERVER-URL USERNAME        password from $RCAD_PASSWORD or one line of stdin
  logout
  whoami
  orgs
  projects ORG-ID
  tree PROJECT-ID
  push FILE --project ID [--folder ID] [--name NAME]     upload as a new document
  save FILE [-m MESSAGE] [--milestone NAME] [--force]   save a linked file as a new version
  pull DOCUMENT-ID [--version N] [--out FILE]
  versions DOCUMENT-ID
  share DOCUMENT-ID [--version N] [--days N] [--download]
  sync                             send versions queued while offline
  project create ORG-ID NAME [--description TEXT]
  members PROJECT-ID               explicit project members and their roles
  member add|set PROJECT-ID USERNAME ROLE    ROLE: viewer, commenter, editor or admin
  member remove PROJECT-ID USERNAME
  mkdir PROJECT-ID NAME [--parent FOLDER-ID]
  trash DOCUMENT-ID                move a document to the project's trash
  restore DOCUMENT-ID              restore a document from the trash
exit codes: 0 ok, 1 error, 3 conflict, 4 locked, 5 offline (queued), 64 usage";

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(64)
}

/// Parsed options after the positional arguments.
struct Opts {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Opts {
    /// Split `args` into positionals and `--flag [value]` pairs; `with_value` lists the flags
    /// that take a value.
    fn parse(args: &[String], with_value: &[&str], switches: &[&str]) -> Option<Opts> {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if with_value.contains(&a.as_str()) {
                flags.push((a.clone(), Some(it.next()?.clone())));
            } else if switches.contains(&a.as_str()) {
                flags.push((a.clone(), None));
            } else if a.starts_with('-') && a.len() > 1 {
                return None;
            } else {
                positional.push(a.clone());
            }
        }
        Some(Opts { positional, flags })
    }

    fn value(&self, names: &[&str]) -> Option<String> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| names.contains(&n.as_str()))
            .and_then(|(_, v)| v.clone())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }
}

struct Cli {
    worker: Worker,
    json: bool,
    /// Path of the last `DocumentReady` printed (for `pull --out`).
    last_ready: Option<PathBuf>,
}

/// Outcome category of a command, from its events.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Ok = 0,
    Error = 1,
    Conflict = 3,
    Locked = 4,
    Queued = 5,
}

pub fn run(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let args: Vec<String> = args.iter().filter(|a| *a != "--json").cloned().collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        return usage();
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (Some(config), Some(cache)) = (
        rcad_cloud::config::default_path(),
        rcad_cloud::cache::default_root(),
    ) else {
        eprintln!("cannot locate the configuration directory (set HOME or XDG_CONFIG_HOME)");
        return ExitCode::FAILURE;
    };
    let mut cli = Cli {
        worker: Worker::new(config, cache),
        json,
        last_ready: None,
    };
    let rest = &args[1..];
    let code = match cmd {
        "login" => cli.login(rest),
        "logout" => cli.simple(rest, 0, |_| Request::SignOut),
        "whoami" | "orgs" => cli.whoami(rest, cmd == "orgs"),
        "projects" => cli.simple(rest, 1, |p| Request::ListProjects { org: p[0].clone() }),
        "tree" => cli.simple(rest, 1, |p| Request::LoadTree {
            project: p[0].clone(),
        }),
        "versions" => cli.simple(rest, 1, |p| Request::LoadDetail {
            document: p[0].clone(),
        }),
        "sync" => cli.simple(rest, 0, |_| Request::SyncPending),
        "push" => cli.push(rest),
        "save" => cli.save(rest),
        "pull" => cli.pull(rest),
        "share" => cli.share(rest),
        "project" => cli.project(rest),
        "members" => cli.members(rest),
        "member" => cli.member(rest),
        "mkdir" => cli.mkdir(rest),
        "trash" => cli.trash(rest, false),
        "restore" => cli.trash(rest, true),
        _ => return usage(),
    };
    match code {
        Some(o) => ExitCode::from(o as u8),
        None => usage(),
    }
}

impl Cli {
    fn exec(&mut self, req: Request) -> (Vec<Event>, Outcome) {
        let mut events = Vec::new();
        self.worker.handle(req, &mut |e| events.push(e));
        let mut outcome = Outcome::Ok;
        for e in &events {
            let o = match e {
                Event::Conflict { .. } => Outcome::Conflict,
                Event::Locked { .. } => Outcome::Locked,
                Event::Error { .. } => Outcome::Error,
                Event::Offline { .. } => Outcome::Queued,
                _ => Outcome::Ok,
            };
            outcome = outcome.max(o);
        }
        (events, outcome)
    }

    /// Run `req` and print its events; Offline counts as an error unless `queues` (a save).
    fn report(&mut self, req: Request, queues: bool) -> Outcome {
        let (events, mut outcome) = self.exec(req);
        let served_from_cache = events.iter().any(|e| {
            matches!(
                e,
                Event::DocumentReady {
                    from_cache: true,
                    ..
                }
            )
        });
        for e in &events {
            self.print(e);
        }
        if outcome == Outcome::Queued && !queues {
            outcome = if served_from_cache {
                Outcome::Ok
            } else {
                Outcome::Error
            };
        }
        outcome
    }

    fn simple(
        &mut self,
        args: &[String],
        n: usize,
        make: impl FnOnce(&[String]) -> Request,
    ) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        if opts.positional.len() != n {
            return None;
        }
        let req = make(&opts.positional);
        // Only `sync` leaves versions queued; elsewhere unreachable is an error.
        let queues = matches!(req, Request::SyncPending);
        Some(self.report(req, queues))
    }

    fn login(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let [url, user] = opts.positional.as_slice() else {
            return None;
        };
        let password = match std::env::var("RCAD_PASSWORD") {
            Ok(p) => p,
            Err(_) => {
                let stdin = std::io::stdin();
                if stdin.is_terminal() {
                    eprint!("Password for {user} (input is echoed; prefer $RCAD_PASSWORD): ");
                    let _ = std::io::stderr().flush();
                }
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).is_err() {
                    eprintln!("cannot read the password from stdin");
                    return Some(Outcome::Error);
                }
                line.trim_end_matches(['\r', '\n']).to_string()
            }
        };
        Some(self.report(
            Request::SignIn {
                server_url: url.clone(),
                username: user.clone(),
                password,
            },
            false,
        ))
    }

    fn whoami(&mut self, args: &[String], orgs_only: bool) -> Option<Outcome> {
        if !args.is_empty() {
            return None;
        }
        let pending = self.worker.pending_count();
        let (events, outcome) = self.exec(Request::Refresh);
        for e in &events {
            match e {
                Event::SignedIn {
                    server_url,
                    server_name,
                    me,
                } if !self.json => {
                    if orgs_only {
                        for o in &me.orgs {
                            println!("{}\t{}\t{:?}", o.id, o.name, o.my_role);
                        }
                    } else {
                        println!(
                            "{} ({}) on {server_name} <{server_url}>",
                            me.user.username, me.user.display_name
                        );
                        println!("pending versions: {pending}");
                    }
                }
                other => self.print(other),
            }
        }
        Some(if outcome == Outcome::Queued {
            Outcome::Error
        } else {
            outcome
        })
    }

    fn push(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &["--project", "--folder", "--name"], &[])?;
        let [file] = opts.positional.as_slice() else {
            return None;
        };
        let project = opts.value(&["--project"])?;
        let path = PathBuf::from(file);
        let name = opts
            .value(&["--name"])
            .unwrap_or_else(|| default_name(&path));
        Some(self.report(
            Request::UploadNew {
                local_path: path,
                project,
                folder: opts.value(&["--folder"]),
                name,
            },
            true,
        ))
    }

    fn save(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &["-m", "--message", "--milestone"], &["--force"])?;
        let [file] = opts.positional.as_slice() else {
            return None;
        };
        Some(self.report(
            Request::SaveVersion {
                local_path: PathBuf::from(file),
                description: opts.value(&["-m", "--message"]).unwrap_or_default(),
                milestone: opts.value(&["--milestone"]),
                force: opts.has("--force"),
            },
            true,
        ))
    }

    fn pull(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &["--version", "--out"], &[])?;
        let [doc] = opts.positional.as_slice() else {
            return None;
        };
        let req = match opts.value(&["--version"]) {
            Some(v) => Request::OpenVersion {
                document: doc.clone(),
                version: v.parse().ok()?,
            },
            None => Request::OpenDocument {
                document: doc.clone(),
            },
        };
        let mut outcome = self.report(req, false);
        if let Some(out) = opts.value(&["--out"])
            && outcome != Outcome::Error
        {
            let src = self.last_ready.take();
            match src {
                Some(src) => match std::fs::copy(&src, &out) {
                    Ok(_) => {
                        if !self.json {
                            println!("copied to {out} (not linked)");
                        }
                    }
                    Err(e) => {
                        eprintln!("cannot write {out}: {e}");
                        outcome = Outcome::Error;
                    }
                },
                None => outcome = Outcome::Error,
            }
        }
        Some(outcome)
    }

    fn share(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &["--days", "--version"], &["--download"])?;
        let [doc] = opts.positional.as_slice() else {
            return None;
        };
        let expires_in_days = match opts.value(&["--days"]) {
            Some(d) => Some(d.parse().ok()?),
            None => None,
        };
        let version = match opts.value(&["--version"]) {
            Some(v) => Some(v.parse().ok()?),
            None => None,
        };
        Some(self.report(
            Request::CreateShare {
                document: doc.clone(),
                version,
                expires_in_days,
                allow_download: opts.has("--download"),
            },
            false,
        ))
    }

    // -- Administration (direct client calls) ---------------------------------------------------

    /// A client for the active account.
    fn client(&self) -> Result<Client, CloudError> {
        let account = self
            .worker
            .active_account()?
            .ok_or(CloudError::NotSignedIn)?;
        Ok(Client::new(&account.server_url)?.with_token(account.token))
    }

    /// Run `call` against the server; print a failure (with `hint` for a 404) and map it to
    /// an exit outcome.
    fn direct<T>(
        &mut self,
        request: &'static str,
        hint_not_found: Option<&str>,
        call: impl FnOnce(&Client) -> Result<T, CloudError>,
    ) -> Result<T, Outcome> {
        let result = self.client().and_then(|c| call(&c));
        result.map_err(|e| {
            let code = e.code();
            let mut message = e.to_string();
            if let (Some(proto::ErrorCode::NotFound), Some(h)) = (code, hint_not_found) {
                message = format!("{message} ({h})");
            }
            self.print(&Event::Error {
                request,
                code,
                message,
            });
            Outcome::Error
        })
    }

    fn emit_json(&self, v: serde_json::Value) {
        println!("{v}");
    }

    fn project(&mut self, args: &[String]) -> Option<Outcome> {
        let (sub, rest) = args.split_first()?;
        if sub != "create" {
            return None;
        }
        let opts = Opts::parse(rest, &["--description"], &[])?;
        let [org, name] = opts.positional.as_slice() else {
            return None;
        };
        let req = proto::CreateProjectRequest {
            name: name.clone(),
            description: opts.value(&["--description"]).unwrap_or_default(),
        };
        Some(
            match self.direct("create_project", None, |c| c.create_project(org, &req)) {
                Ok(p) => {
                    if self.json {
                        self.emit_json(json!({"event": "project_created", "project": to_json(&p)}));
                    } else {
                        println!("{}\t{}\t{}", p.id, p.name, role_name(p.my_role));
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            },
        )
    }

    fn members(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let [project] = opts.positional.as_slice() else {
            return None;
        };
        Some(
            match self.direct("project_members", None, |c| c.project_members(project)) {
                Ok(members) => {
                    if self.json {
                        self.emit_json(json!({
                            "event": "project_members", "project": project,
                            "members": to_json(&members)
                        }));
                    } else {
                        for m in &members {
                            println!(
                                "{}\t{}\t{}",
                                m.user.username,
                                role_name(m.role),
                                m.user.display_name
                            );
                        }
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            },
        )
    }

    fn member(&mut self, args: &[String]) -> Option<Outcome> {
        let (sub, rest) = args.split_first()?;
        let opts = Opts::parse(rest, &[], &[])?;
        match (sub.as_str(), opts.positional.as_slice()) {
            ("add" | "set", [project, username, role]) => {
                let req = proto::SetProjectMemberRequest {
                    username: username.clone(),
                    role: parse_role(role)?,
                };
                let hint = format!(
                    "the project is not visible to you, or {username} is not a member of the \
                     project's organization; add them to the organization first"
                );
                Some(
                    match self.direct("set_project_member", Some(&hint), |c| {
                        c.set_project_member(project, &req)
                    }) {
                        Ok(m) => {
                            if self.json {
                                self.emit_json(json!({
                                    "event": "project_member_set", "project": project,
                                    "member": to_json(&m)
                                }));
                            } else {
                                println!(
                                    "{} is {} of {project}",
                                    m.user.username,
                                    role_name(m.role)
                                );
                            }
                            Outcome::Ok
                        }
                        Err(o) => o,
                    },
                )
            }
            ("remove", [project, username]) => Some(
                match self.direct("remove_project_member", None, |c| {
                    c.remove_project_member(project, username)
                }) {
                    Ok(()) => {
                        if self.json {
                            self.emit_json(json!({
                                "event": "project_member_removed", "project": project,
                                "username": username
                            }));
                        } else {
                            println!("removed {username} from {project}");
                        }
                        Outcome::Ok
                    }
                    Err(o) => o,
                },
            ),
            _ => None,
        }
    }

    fn mkdir(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &["--parent"], &[])?;
        let [project, name] = opts.positional.as_slice() else {
            return None;
        };
        let req = proto::CreateFolderRequest {
            name: name.clone(),
            parent: opts.value(&["--parent"]),
        };
        Some(
            match self.direct("create_folder", None, |c| c.create_folder(project, &req)) {
                Ok(f) => {
                    if self.json {
                        self.emit_json(json!({"event": "folder_created", "folder": to_json(&f)}));
                    } else {
                        println!("{}\t{}", f.id, f.name);
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            },
        )
    }

    /// `trash` (restore = false) or `restore` a document.
    fn trash(&mut self, args: &[String], restore: bool) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let [doc] = opts.positional.as_slice() else {
            return None;
        };
        let outcome = if restore {
            match self.direct("restore_document", None, |c| c.restore_document(doc)) {
                Ok(d) => {
                    if self.json {
                        self.emit_json(
                            json!({"event": "document_restored", "document": to_json(&d)}),
                        );
                    } else {
                        println!("restored {} ({})", d.name, d.id);
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            }
        } else {
            match self.direct("trash_document", None, |c| c.delete_document(doc)) {
                Ok(()) => {
                    if self.json {
                        self.emit_json(json!({"event": "document_trashed", "document": doc}));
                    } else {
                        println!(
                            "moved {doc} to the trash (restore with `rcad-cli cloud restore {doc}`)"
                        );
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            }
        };
        Some(outcome)
    }

    fn print(&mut self, e: &Event) {
        if let Event::DocumentReady { path, .. } = e {
            self.last_ready = Some(path.clone());
        }
        if self.json {
            println!("{}", event_json(e));
            return;
        }
        match e {
            Event::SignedIn {
                server_url,
                server_name,
                me,
            } => {
                println!(
                    "signed in to {server_name} <{server_url}> as {}",
                    me.user.username
                );
                for o in &me.orgs {
                    println!("  org {}\t{}\t{:?}", o.id, o.name, o.my_role);
                }
            }
            Event::SignedOut => println!("signed out"),
            Event::Projects { projects, .. } => {
                for p in projects {
                    println!("{}\t{}\t{:?}", p.id, p.name, p.my_role);
                }
            }
            Event::Tree(t) => print_tree(t),
            Event::DocumentReady {
                path,
                link,
                read_only,
                from_cache,
            } => {
                let mut notes = Vec::new();
                if *read_only {
                    notes.push("read-only");
                }
                if *from_cache {
                    notes.push("cached copy");
                }
                let notes = if notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", notes.join(", "))
                };
                println!(
                    "{} version {}: {}{notes}",
                    link.document_name,
                    link.base_version,
                    path.display()
                );
            }
            Event::VersionSaved { link, version } => println!(
                "saved {} version {} ({} bytes) from {}",
                link.document_name,
                version.number,
                version.size,
                link.local_path.display()
            ),
            Event::Conflict {
                local_path,
                base_version,
                head_version,
                ..
            } => eprintln!(
                "conflict: {} is based on version {base_version} but the server is at version {head_version}; \
                 save with --force to save anyway, or push it as a new document",
                local_path.display()
            ),
            Event::Locked { lock, .. } => {
                eprintln!(
                    "locked: {} holds the edit lock (expires {})",
                    lock.holder,
                    time(lock.expires_at)
                )
            }
            Event::LockAcquired { lock, .. } => {
                println!("lock held until {}", time(lock.expires_at))
            }
            Event::LockReleased { .. } => println!("lock released"),
            Event::Detail(d) => print_detail(d),
            Event::ShareCreated(s) => {
                println!("{}", s.url);
                if let Some(t) = s.link.expires_at {
                    println!("expires {}", time(t));
                }
            }
            Event::Comments { comments, .. } => {
                for c in comments {
                    let done = if c.resolved { " [resolved]" } else { "" };
                    println!("{}\tv{}\t{}: {}{done}", c.id, c.version, c.author, c.body);
                }
            }
            Event::Offline { pending } => eprintln!(
                "server unreachable; {pending} version(s) queued (run `rcad-cli cloud sync` later)"
            ),
            Event::Synced { pending } => println!("pending versions: {pending}"),
            Event::PendingQueue { saves } => {
                for (i, p) in saves.iter().enumerate() {
                    println!(
                        "{i}\t{}\t{}\t{}",
                        p.document,
                        time(p.queued_at),
                        p.description
                    );
                }
            }
            Event::ProjectMembers { members, .. } => {
                for m in members {
                    println!("{}\t{:?}\t{}", m.user.username, m.role, m.user.display_name);
                }
            }
            Event::OrgMembers { members, .. } => {
                for m in members {
                    println!("{}\t{:?}\t{}", m.user.username, m.role, m.user.display_name);
                }
            }
            Event::Usage { usage, .. } => println!(
                "{} bytes used of {}; {} documents, {} versions",
                usage.used_bytes,
                usage
                    .quota_bytes
                    .map_or("no quota".to_string(), |q| format!("{q} bytes")),
                usage.documents,
                usage.versions
            ),
            Event::Error {
                request, message, ..
            } => eprintln!("error ({request}): {message}"),
        }
    }
}

/// A project role from its name, case-insensitively.
fn parse_role(s: &str) -> Option<proto::ProjectRole> {
    match s.to_ascii_lowercase().as_str() {
        "viewer" => Some(proto::ProjectRole::Viewer),
        "commenter" => Some(proto::ProjectRole::Commenter),
        "editor" => Some(proto::ProjectRole::Editor),
        "admin" => Some(proto::ProjectRole::Admin),
        _ => None,
    }
}

fn role_name(r: proto::ProjectRole) -> &'static str {
    match r {
        proto::ProjectRole::Viewer => "viewer",
        proto::ProjectRole::Commenter => "commenter",
        proto::ProjectRole::Editor => "editor",
        proto::ProjectRole::Admin => "admin",
    }
}

fn default_name(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = file
        .strip_suffix(".rcad.json")
        .or_else(|| file.strip_suffix(".json"))
        .unwrap_or(&file)
        .to_string();
    if stem.is_empty() {
        "Untitled".into()
    } else {
        stem
    }
}

fn print_tree(t: &proto::ProjectTree) {
    println!("project {}\t{}", t.project.id, t.project.name);
    fn docs_in(t: &proto::ProjectTree, folder: Option<&str>, depth: usize) {
        let pad = "  ".repeat(depth);
        for f in t.folders.iter().filter(|f| f.parent.as_deref() == folder) {
            println!("{pad}{}/\t{}", f.name, f.id);
            if depth < 64 {
                docs_in(t, Some(&f.id), depth + 1);
            }
        }
        for d in t.documents.iter().filter(|d| d.folder.as_deref() == folder) {
            let lock = d
                .lock
                .as_ref()
                .map(|l| format!("\tlocked by {}", l.holder))
                .unwrap_or_default();
            println!("{pad}{}\t{}\tv{}{lock}", d.name, d.id, d.head_version);
        }
    }
    docs_in(t, None, 1);
    if !t.trash.is_empty() {
        println!("  trash:");
        for d in &t.trash {
            println!("    {}\t{}", d.name, d.id);
        }
    }
}

fn print_detail(d: &proto::DocumentDetail) {
    println!(
        "{}\t{}\thead v{}",
        d.document.name, d.document.id, d.document.head_version
    );
    for v in &d.versions {
        let star = v
            .milestone
            .as_ref()
            .map(|m| format!("\t* {m}"))
            .unwrap_or_default();
        let from = v
            .promoted_from
            .map(|n| format!("\t(from v{n})"))
            .unwrap_or_default();
        println!(
            "v{}\t{}\t{}\t{}{star}{from}",
            v.number,
            time(v.created_at),
            v.author,
            v.description
        );
    }
}

/// `YYYY-MM-DD HH:MM UTC` from Unix seconds.
fn time(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

fn link_json(l: &Link) -> serde_json::Value {
    serde_json::to_value(l).unwrap_or(serde_json::Value::Null)
}

fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// One event as a JSON object `{"event": name, ...}`.
fn event_json(e: &Event) -> serde_json::Value {
    match e {
        Event::SignedIn {
            server_url,
            server_name,
            me,
        } => {
            json!({"event": "signed_in", "server_url": server_url, "server_name": server_name, "me": to_json(me)})
        }
        Event::SignedOut => json!({"event": "signed_out"}),
        Event::Projects { org, projects } => {
            json!({"event": "projects", "org": org, "projects": to_json(projects)})
        }
        Event::Tree(t) => json!({"event": "tree", "tree": to_json(t)}),
        Event::DocumentReady {
            path,
            link,
            read_only,
            from_cache,
        } => json!({
            "event": "document_ready", "path": path, "link": link_json(link),
            "read_only": read_only, "from_cache": from_cache
        }),
        Event::VersionSaved { link, version } => {
            json!({"event": "version_saved", "link": link_json(link), "version": to_json(version)})
        }
        Event::Conflict {
            local_path,
            document,
            base_version,
            head_version,
        } => json!({
            "event": "conflict", "local_path": local_path, "document": document,
            "base_version": base_version, "head_version": head_version
        }),
        Event::Locked { document, lock } => {
            json!({"event": "locked", "document": document, "lock": to_json(lock)})
        }
        Event::LockAcquired { document, lock } => {
            json!({"event": "lock_acquired", "document": document, "lock": to_json(lock)})
        }
        Event::LockReleased { document } => json!({"event": "lock_released", "document": document}),
        Event::Detail(d) => json!({"event": "detail", "detail": to_json(d)}),
        Event::ShareCreated(s) => json!({"event": "share_created", "share": to_json(s)}),
        Event::Comments { document, comments } => {
            json!({"event": "comments", "document": document, "comments": to_json(comments)})
        }
        Event::Offline { pending } => json!({"event": "offline", "pending": pending}),
        Event::Synced { pending } => json!({"event": "synced", "pending": pending}),
        Event::PendingQueue { saves } => json!({"event": "pending_queue", "saves": to_json(saves)}),
        Event::ProjectMembers { project, members } => {
            json!({"event": "project_members", "project": project, "members": to_json(members)})
        }
        Event::OrgMembers { org, members } => {
            json!({"event": "org_members", "org": org, "members": to_json(members)})
        }
        Event::Usage { org, usage } => {
            json!({"event": "usage", "org": org, "usage": to_json(usage)})
        }
        Event::Error {
            request,
            code,
            message,
        } => json!({
            "event": "error", "request": request, "code": code.map(|c| to_json(&c)), "message": message
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil_utc() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(time(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(time(1_791_000_000), "2026-10-03 04:00 UTC");
    }

    #[test]
    fn option_parsing() {
        let a: Vec<String> = ["f.rcad.json", "-m", "msg", "--force"]
            .map(String::from)
            .to_vec();
        let o = Opts::parse(&a, &["-m"], &["--force"]).unwrap();
        assert_eq!(o.positional, vec!["f.rcad.json"]);
        assert_eq!(o.value(&["-m"]).as_deref(), Some("msg"));
        assert!(o.has("--force"));
        assert!(Opts::parse(&["--bogus".to_string()], &[], &[]).is_none());
        assert!(Opts::parse(&["-m".to_string()], &["-m"], &[]).is_none());
        assert_eq!(default_name(Path::new("/x/Bracket.rcad.json")), "Bracket");
    }

    #[test]
    fn roles_parse_case_insensitively() {
        assert_eq!(parse_role("Editor"), Some(proto::ProjectRole::Editor));
        assert_eq!(parse_role("ADMIN"), Some(proto::ProjectRole::Admin));
        assert_eq!(parse_role("viewer"), Some(proto::ProjectRole::Viewer));
        assert_eq!(parse_role("commenter"), Some(proto::ProjectRole::Commenter));
        assert_eq!(parse_role("owner"), None);
        for r in [
            proto::ProjectRole::Viewer,
            proto::ProjectRole::Commenter,
            proto::ProjectRole::Editor,
            proto::ProjectRole::Admin,
        ] {
            assert_eq!(parse_role(role_name(r)), Some(r));
        }
    }
}
