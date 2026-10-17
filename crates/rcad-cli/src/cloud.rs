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
//!
//! Text output passes through [`term_safe`]: names, descriptions, comments, usernames and
//! error messages come from the server (and other users), so control characters, escape
//! sequences and bidirectional-override characters are shown escaped instead of reaching
//! the terminal. `--json` output is plain JSON from serde.
use rcad_cloud::CloudError;
use rcad_cloud::cache::Link;
use rcad_cloud::client::Client;
use rcad_cloud::proto;
use rcad_cloud::service::{Event, Request, Worker};
use serde_json::json;
use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `s` with every character that could control a terminal shown as a `\u{..}` escape: C0
/// controls except tab (so also newlines, which could forge extra output lines), DEL, C1
/// controls (8-bit CSI and OSC included) and the bidirectional controls that reorder what
/// is displayed (U+061C, U+200E, U+200F, U+202A..U+202E, U+2066..U+2069).
pub(crate) fn term_safe(s: &str) -> std::borrow::Cow<'_, str> {
    fn bad(c: char) -> bool {
        (c.is_control() && c != '\t')
            || matches!(
                c,
                '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    }
    if !s.chars().any(bad) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if bad(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// A line of text output, through [`term_safe`] and [`crate::out::line`].
macro_rules! outln {
    ($($arg:tt)*) => {
        crate::out::line(&term_safe(&format!($($arg)*)))
    };
}

/// A line on standard error, through [`term_safe`] and [`crate::out::eline`].
macro_rules! errln {
    ($($arg:tt)*) => {
        crate::out::eline(&term_safe(&format!($($arg)*)))
    };
}

const USAGE: &str = "\
usage: rcad-cli cloud [--json] [--account SERVER-URL[#USERNAME]] COMMAND
  --json and --account may appear anywhere after `cloud` (or before it). --account uses
  that account for this one command instead of the active one.
  login SERVER-URL USERNAME [--allow-insecure-http]
                                   password from $RCAD_PASSWORD, a no-echo prompt on a
                                   terminal, or one line of stdin; plain http:// to a host
                                   not on a local network needs --allow-insecure-http
  logout                           sign out of the active account (or --account's)
  whoami
  accounts                         list signed-in accounts; * marks the active one
  use SERVER-URL [USERNAME]        make that account the active one
  orgs
  projects ORG-ID
  tree PROJECT-ID
  push FILE --project ID [--folder ID] [--name NAME] [--thumbnail PNG]
                                   upload as a new document; prints its id
  save FILE [-m MESSAGE] [--milestone NAME] [--force] [--thumbnail PNG]
                                   save a linked file as a new version (PNG: at most 1 MiB)
  pull DOCUMENT-ID [--version N] [--out FILE]
  versions DOCUMENT-ID
  share DOCUMENT-ID [--version N] [--days N] [--download]
  shares DOCUMENT-ID               list the document's share links
  activity PROJECT-ID              the project's recent activity, newest first
  unshare SHARE-ID                 revoke a share link (it stops working at once)
  sync                             send versions queued while offline
  queue                            list versions queued while offline (INDEX first)
  discard INDEX [--yes]            drop a queued version (the local file is not changed);
                                   asks first on a terminal, needs --yes otherwise
  project create ORG-ID NAME [--description TEXT]
  members PROJECT-ID               explicit project members and their roles
  member add|set PROJECT-ID USERNAME ROLE    ROLE: viewer, commenter, editor or admin
  member remove PROJECT-ID USERNAME
  mkdir PROJECT-ID NAME [--parent FOLDER-ID]
  trash DOCUMENT-ID                move a document to the project's trash
  restore DOCUMENT-ID              restore a document from the trash
exit codes: 0 ok, 1 error, 3 conflict, 4 locked, 5 offline (queued), 64 usage,
  141 standard output closed early (for example by `| head`)";

fn usage() -> ExitCode {
    crate::out::eline(USAGE);
    ExitCode::from(64)
}

/// True for the options every `cloud` command takes (`--json`, `--account URL`,
/// `--account=URL`).
pub fn is_global_option(a: &str) -> bool {
    a == "--json" || a == "--account" || a.starts_with("--account=")
}

/// Options every `cloud` command takes, removed from the arguments wherever they appear.
struct Globals {
    json: bool,
    /// `--account SERVER-URL[#USERNAME]`.
    account: Option<(String, Option<String>)>,
}

/// Split `args` into [`Globals`] and the rest; None for `--account` without a value or with
/// an empty server or username.
fn split_globals(args: &[String]) -> Option<(Globals, Vec<String>)> {
    let mut g = Globals {
        json: false,
        account: None,
    };
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let account = if a == "--json" {
            g.json = true;
            continue;
        } else if a == "--account" {
            it.next()?.clone()
        } else if let Some(v) = a.strip_prefix("--account=") {
            v.to_string()
        } else {
            rest.push(a.clone());
            continue;
        };
        g.account = Some(parse_account_ref(&account)?);
    }
    Some((g, rest))
}

/// `SERVER-URL[#USERNAME]` (a server URL never contains `#`).
fn parse_account_ref(s: &str) -> Option<(String, Option<String>)> {
    let (url, user) = match s.rsplit_once('#') {
        Some((u, n)) => (u, Some(n)),
        None => (s, None),
    };
    if url.trim().is_empty() || user.is_some_and(|u| u.is_empty()) {
        return None;
    }
    Some((url.to_string(), user.map(str::to_string)))
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
    Usage = 64,
}

pub fn run(args: &[String]) -> ExitCode {
    let Some((globals, args)) = split_globals(args) else {
        return usage();
    };
    let json = globals.json;
    let Some(cmd) = args.first().map(String::as_str) else {
        return usage();
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        crate::out::line(USAGE);
        return ExitCode::SUCCESS;
    }
    // These name their account themselves.
    if globals.account.is_some() && matches!(cmd, "login" | "accounts" | "use") {
        errln!("--account cannot be used with `{cmd}`");
        return usage();
    }
    let (Some(config), Some(cache)) = (
        rcad_cloud::config::default_path(),
        rcad_cloud::cache::default_root(),
    ) else {
        errln!("cannot locate the configuration directory (set HOME or XDG_CONFIG_HOME)");
        return ExitCode::FAILURE;
    };
    let mut worker = Worker::new(config, cache);
    if let Some((url, user)) = globals.account {
        worker.use_account(url, user);
    }
    let mut cli = Cli {
        worker,
        json,
        last_ready: None,
    };
    let rest = &args[1..];
    let code = match cmd {
        "login" => cli.login(rest),
        "logout" => cli.simple(rest, 0, |_| Request::SignOut),
        "whoami" | "orgs" => cli.whoami(rest, cmd == "orgs"),
        "accounts" => cli.accounts(rest),
        "use" => cli.use_account(rest),
        "shares" => cli.shares(rest),
        "unshare" => cli.unshare(rest),
        "projects" => cli.simple(rest, 1, |p| Request::ListProjects { org: p[0].clone() }),
        "tree" => cli.simple(rest, 1, |p| Request::LoadTree {
            project: p[0].clone(),
        }),
        "versions" => cli.simple(rest, 1, |p| Request::LoadDetail {
            document: p[0].clone(),
        }),
        "sync" => cli.simple(rest, 0, |_| Request::SyncPending),
        "queue" => cli.simple(rest, 0, |_| Request::LoadPending),
        "activity" => cli.simple(rest, 1, |p| Request::LoadActivity {
            project: p[0].clone(),
        }),
        "discard" => cli.discard(rest),
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
        let opts = Opts::parse(args, &[], &["--allow-insecure-http"])?;
        let [url, user] = opts.positional.as_slice() else {
            return None;
        };
        let allow_insecure_http = opts.has("--allow-insecure-http");
        // Refuse before asking for the password.
        if let Ok(normalized) = rcad_cloud::client::normalize_server_url(url)
            && rcad_cloud::client::needs_insecure_consent(&normalized)
        {
            if !allow_insecure_http {
                errln!(
                    "error (sign_in): {normalized} is not encrypted. {} Pass \
                     --allow-insecure-http to connect anyway.",
                    rcad_cloud::client::INSECURE_HTTP_WARNING
                );
                return Some(Outcome::Error);
            }
            errln!(
                "warning: {normalized} is not encrypted. {}",
                rcad_cloud::client::INSECURE_HTTP_WARNING
            );
        }
        let password = match std::env::var("RCAD_PASSWORD") {
            Ok(p) => p,
            Err(_) if std::io::stdin().is_terminal() => {
                // Reads from the terminal with echo off.
                match rpassword::prompt_password(format!("Password for {}: ", term_safe(user))) {
                    Ok(p) => p,
                    Err(e) => {
                        errln!("cannot read the password: {e}");
                        return Some(Outcome::Error);
                    }
                }
            }
            Err(_) => {
                let mut line = String::new();
                if std::io::stdin().lock().read_line(&mut line).is_err() {
                    errln!("cannot read the password from stdin");
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
                allow_insecure_http,
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
        // Offline: still say which account is in use (from the account file).
        if outcome == Outcome::Queued
            && !self.json
            && !orgs_only
            && let Ok(Some(a)) = self.worker.active_account()
        {
            outln!(
                "{} on {} <{}> (server unreachable)",
                a.username,
                a.server_name,
                a.server_url
            );
            outln!("pending versions: {pending}");
        }
        for e in &events {
            match e {
                Event::SignedIn {
                    server_url,
                    server_name,
                    me,
                } if !self.json => {
                    if orgs_only {
                        for o in &me.orgs {
                            outln!("{}\t{}\t{:?}", o.id, o.name, o.my_role);
                        }
                    } else {
                        outln!(
                            "{} ({}) on {server_name} <{server_url}>",
                            me.user.username,
                            me.user.display_name
                        );
                        outln!("pending versions: {pending}");
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
        let opts = Opts::parse(
            args,
            &["--project", "--folder", "--name", "--thumbnail"],
            &[],
        )?;
        let [file] = opts.positional.as_slice() else {
            return None;
        };
        let project = opts.value(&["--project"])?;
        let path = PathBuf::from(file);
        let name = opts
            .value(&["--name"])
            .unwrap_or_else(|| default_name(&path));
        let thumbnail = match read_thumbnail(&opts) {
            Ok(t) => t,
            Err(o) => return Some(o),
        };
        let (events, outcome) = self.exec(Request::UploadNew {
            local_path: path,
            project,
            folder: opts.value(&["--folder"]),
            name,
            thumbnail,
        });
        for e in &events {
            self.print(e);
            // Every later command (versions, share, pull) needs the new id. (`--json` has it
            // in the event's `document` field.)
            if let Event::VersionSaved { link, .. } = e
                && !self.json
            {
                outln!("document {}", link.document);
            }
        }
        Some(outcome)
    }

    fn save(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(
            args,
            &["-m", "--message", "--milestone", "--thumbnail"],
            &["--force"],
        )?;
        let [file] = opts.positional.as_slice() else {
            return None;
        };
        let thumbnail = match read_thumbnail(&opts) {
            Ok(t) => t,
            Err(o) => return Some(o),
        };
        Some(self.report(
            Request::SaveVersion {
                local_path: PathBuf::from(file),
                description: opts.value(&["-m", "--message"]).unwrap_or_default(),
                milestone: opts.value(&["--milestone"]),
                force: opts.has("--force"),
                thumbnail,
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
                            outln!("copied to {out} (not linked)");
                        }
                    }
                    Err(e) => {
                        errln!("cannot write {out}: {e}");
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
        crate::out::line(&v.to_string());
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
                        outln!("{}\t{}\t{}", p.id, p.name, role_name(p.my_role));
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
                            outln!(
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
                                outln!("{} is {} of {project}", m.user.username, role_name(m.role));
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
                            outln!("removed {username} from {project}");
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
                        outln!("{}\t{}", f.id, f.name);
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            },
        )
    }

    /// The document's share links.
    fn shares(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let [doc] = opts.positional.as_slice() else {
            return None;
        };
        Some(match self.direct("list_shares", None, |c| c.shares(doc)) {
            Ok(links) => {
                if self.json {
                    self.emit_json(
                        json!({"event": "shares", "document": doc, "shares": to_json(&links)}),
                    );
                } else {
                    for l in &links {
                        outln!("{}", share_line(l));
                    }
                }
                Outcome::Ok
            }
            Err(o) => o,
        })
    }

    /// Revoke a share link.
    fn unshare(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let [share] = opts.positional.as_slice() else {
            return None;
        };
        Some(
            match self.direct(
                "revoke_share",
                Some("see `rcad-cli cloud shares DOCUMENT-ID` for the link ids"),
                |c| c.delete_share(share),
            ) {
                Ok(()) => {
                    if self.json {
                        self.emit_json(json!({"event": "share_revoked", "share": share}));
                    } else {
                        outln!("revoked share link {share}");
                    }
                    Outcome::Ok
                }
                Err(o) => o,
            },
        )
    }

    /// The signed-in accounts, the active one marked. Local only.
    fn accounts(&mut self, args: &[String]) -> Option<Outcome> {
        if !args.is_empty() {
            return None;
        }
        let config = match rcad_cloud::config::CloudConfig::load(self.worker.config_path()) {
            Ok(c) => c,
            Err(e) => return Some(self.local_error("accounts", e)),
        };
        if self.json {
            let list: Vec<_> = config
                .accounts
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    json!({
                        "server_url": a.server_url, "server_name": a.server_name,
                        "username": a.username, "active": config.active == Some(i)
                    })
                })
                .collect();
            self.emit_json(json!({"event": "accounts", "accounts": list}));
        } else {
            for (i, a) in config.accounts.iter().enumerate() {
                let mark = if config.active == Some(i) { "*" } else { " " };
                outln!("{mark} {}\t{}\t{}", a.server_url, a.username, a.server_name);
            }
        }
        Some(Outcome::Ok)
    }

    /// Make an account the active one. Local only.
    fn use_account(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &[])?;
        let (url, user) = match opts.positional.as_slice() {
            [url] => (url, None),
            [url, user] => (url, Some(user.as_str())),
            _ => return None,
        };
        let result = rcad_cloud::config::CloudConfig::update(self.worker.config_path(), |c| {
            c.find_account(url, user).map(|i| {
                c.active = Some(i);
                c.accounts[i].clone()
            })
        })
        .and_then(|r| r);
        Some(match result {
            Ok(a) => {
                if self.json {
                    self.emit_json(json!({
                        "event": "account_selected", "server_url": a.server_url,
                        "server_name": a.server_name, "username": a.username
                    }));
                } else {
                    outln!(
                        "using {} on {} <{}>",
                        a.username,
                        a.server_name,
                        a.server_url
                    );
                }
                Outcome::Ok
            }
            Err(e) => self.local_error("use", e),
        })
    }

    /// Print a local failure as an error event.
    fn local_error(&mut self, request: &'static str, e: CloudError) -> Outcome {
        self.print(&Event::Error {
            request,
            code: e.code(),
            message: e.to_string(),
        });
        Outcome::Error
    }

    /// Drop the queued save at INDEX (as `queue` lists it), after confirmation: a prompt on a
    /// terminal, `--yes` otherwise.
    fn discard(&mut self, args: &[String]) -> Option<Outcome> {
        let opts = Opts::parse(args, &[], &["--yes", "-y"])?;
        let [index] = opts.positional.as_slice() else {
            return None;
        };
        let index: usize = index.parse().ok()?;
        let confirmed = opts.has("--yes") || opts.has("-y");
        let entry = self
            .worker
            .cache()
            .ok()
            .flatten()
            .and_then(|c| c.pending().ok())
            .and_then(|q| q.get(index).cloned());
        let Some(entry) = entry else {
            errln!(
                "error (discard_pending): no queued version {index} (see `rcad-cli cloud queue`)"
            );
            return Some(Outcome::Error);
        };
        let what = format!(
            "{index}: {} queued {} {}",
            entry.document,
            time(entry.queued_at),
            entry.description
        );
        if !confirmed {
            let stdin = std::io::stdin();
            if !stdin.is_terminal() {
                errln!(
                    "discard {what}\nrefusing without confirmation: run it on a terminal, or \
                     add --yes (`rcad-cli cloud discard {index} --yes`)"
                );
                return Some(Outcome::Usage);
            }
            errln!("discard queued version {what}");
            crate::out::prompt("the version is lost for good; discard it? [y/N] ");
            let mut answer = String::new();
            if stdin.lock().read_line(&mut answer).is_err() || !is_yes(&answer) {
                errln!("not discarded");
                return Some(Outcome::Error);
            }
        }
        let (events, outcome) = self.exec(Request::DiscardPending {
            index,
            document: entry.document.clone(),
            blob: entry.blob,
        });
        let ok = !events.iter().any(|e| matches!(e, Event::Error { .. }));
        if ok {
            if self.json {
                self.emit_json(json!({
                    "event": "discarded", "index": index, "document": entry.document,
                    "description": entry.description
                }));
            } else {
                outln!("discarded {what}");
            }
        }
        for e in &events {
            // Text: the "discarded" line replaces the reprinted queue.
            if !self.json && ok && matches!(e, Event::PendingQueue { .. }) {
                continue;
            }
            self.print(e);
        }
        Some(if outcome == Outcome::Queued {
            Outcome::Error
        } else {
            outcome
        })
    }

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
                        outln!("restored {} ({})", d.name, d.id);
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
                        outln!(
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
            crate::out::line(&event_json(e).to_string());
            return;
        }
        match e {
            Event::SignedIn {
                server_url,
                server_name,
                me,
            } => {
                outln!(
                    "signed in to {server_name} <{server_url}> as {}",
                    me.user.username
                );
                for o in &me.orgs {
                    outln!("  org {}\t{}\t{:?}", o.id, o.name, o.my_role);
                }
            }
            Event::SignedOut => outln!("signed out"),
            Event::Projects { projects, .. } => {
                for p in projects {
                    outln!("{}\t{}\t{:?}", p.id, p.name, p.my_role);
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
                outln!(
                    "{} version {}: {}{notes}",
                    link.document_name,
                    link.base_version,
                    path.display()
                );
            }
            Event::VersionSaved { link, version } => outln!(
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
            } => errln!(
                "conflict: {} is based on version {base_version} but the server is at version {head_version}; \
                 save with --force to save anyway, or push it as a new document",
                local_path.display()
            ),
            Event::Locked { lock, .. } => {
                errln!(
                    "locked: {} holds the edit lock (expires {})",
                    lock.holder,
                    time(lock.expires_at)
                )
            }
            Event::LockAcquired { lock, .. } => {
                outln!("lock held until {}", time(lock.expires_at))
            }
            Event::LockReleased { .. } => outln!("lock released"),
            Event::Detail(d) => print_detail(d),
            Event::Thumbnail {
                document,
                version,
                png,
            } => match png {
                Some(p) => outln!("{document} v{version}: thumbnail, {} bytes", p.len()),
                None => outln!("{document} v{version}: no thumbnail"),
            },
            Event::Head {
                document,
                head_version,
            } => outln!("{document} head v{head_version}"),
            Event::ShareCreated(s) => {
                outln!("{}", s.url);
                if let Some(t) = s.link.expires_at {
                    outln!("expires {}", time(t));
                }
            }
            Event::Comments { comments, .. } => {
                for c in comments {
                    let done = if c.resolved { " [resolved]" } else { "" };
                    outln!("{}\tv{}\t{}: {}{done}", c.id, c.version, c.author, c.body);
                }
            }
            Event::Offline { pending } => errln!(
                "server unreachable; {pending} version(s) queued (run `rcad-cli cloud sync` later)"
            ),
            Event::Synced { pending } => outln!("pending versions: {pending}"),
            Event::PendingQueue { saves } => {
                for (i, p) in saves.iter().enumerate() {
                    outln!(
                        "{i}\t{}\t{}\t{}",
                        p.document,
                        time(p.queued_at),
                        p.description
                    );
                }
            }
            Event::ProjectMembers { members, .. } => {
                for m in members {
                    outln!("{}\t{:?}\t{}", m.user.username, m.role, m.user.display_name);
                }
            }
            Event::OrgMembers { members, .. } => {
                for m in members {
                    outln!("{}\t{:?}\t{}", m.user.username, m.role, m.user.display_name);
                }
            }
            Event::Shares { shares, .. } => {
                for l in shares {
                    outln!("{}", share_line(l));
                }
            }
            Event::Activity { entries, .. } | Event::OrgActivity { entries, .. } => {
                for a in entries {
                    outln!("{}\t{}\t{}\t{}", time(a.at), a.actor, a.action, a.summary);
                }
            }
            Event::Usage { usage, .. } => outln!(
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
            } => errln!("error ({request}): {message}"),
        }
    }
}

/// `y` or `yes`, any case.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// One share link as a line: id, version (or `latest`), expiry, download, creator.
fn share_line(l: &proto::ShareLink) -> String {
    format!(
        "{}\t{}\t{}\t{}\tby {}",
        l.id,
        l.version.map_or("latest".to_string(), |v| format!("v{v}")),
        l.expires_at
            .map_or("no expiry".to_string(), |t| format!("expires {}", time(t))),
        if l.allow_download {
            "download"
        } else {
            "view only"
        },
        l.created_by
    )
}

/// The `--thumbnail` PNG, checked (signature, size) before anything is sent.
fn read_thumbnail(opts: &Opts) -> Result<Option<Vec<u8>>, Outcome> {
    let Some(path) = opts.value(&["--thumbnail"]) else {
        return Ok(None);
    };
    let fail = |m: String| {
        errln!("error (thumbnail): {m}");
        Outcome::Error
    };
    let meta = std::fs::metadata(&path).map_err(|e| fail(format!("cannot read {path}: {e}")))?;
    if meta.len() > proto::MAX_THUMBNAIL_BYTES {
        return Err(fail(format!(
            "{path} is {} bytes, more than the {}-byte limit",
            meta.len(),
            proto::MAX_THUMBNAIL_BYTES
        )));
    }
    let png = std::fs::read(&path).map_err(|e| fail(format!("cannot read {path}: {e}")))?;
    rcad_cloud::client::check_thumbnail(&png).map_err(|e| fail(format!("{path}: {e}")))?;
    Ok(Some(png))
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
    outln!("project {}\t{}", t.project.id, t.project.name);
    fn docs_in(t: &proto::ProjectTree, folder: Option<&str>, depth: usize) {
        let pad = "  ".repeat(depth);
        for f in t.folders.iter().filter(|f| f.parent.as_deref() == folder) {
            outln!("{pad}{}/\t{}", f.name, f.id);
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
            outln!("{pad}{}\t{}\tv{}{lock}", d.name, d.id, d.head_version);
        }
    }
    docs_in(t, None, 1);
    if !t.trash.is_empty() {
        outln!("  trash:");
        for d in &t.trash {
            outln!("    {}\t{}", d.name, d.id);
        }
    }
}

fn print_detail(d: &proto::DocumentDetail) {
    outln!(
        "{}\t{}\thead v{}",
        d.document.name,
        d.document.id,
        d.document.head_version
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
        outln!(
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
        Event::VersionSaved { link, version } => json!({
            "event": "version_saved", "document": link.document, "link": link_json(link),
            "version": to_json(version)
        }),
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
        Event::Thumbnail {
            document,
            version,
            png,
        } => json!({
            "event": "thumbnail", "document": document, "version": version,
            "bytes": png.as_ref().map(Vec::len)
        }),
        Event::Head {
            document,
            head_version,
        } => json!({"event": "head", "document": document, "head_version": head_version}),
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
        Event::Shares { document, shares } => {
            json!({"event": "shares", "document": document, "shares": to_json(shares)})
        }
        Event::Activity { project, entries } => {
            json!({"event": "activity", "project": project, "entries": to_json(entries)})
        }
        Event::OrgActivity { org, entries } => {
            json!({"event": "org_activity", "org": org, "entries": to_json(entries)})
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
    fn hostile_server_text_cannot_reach_the_terminal() {
        let cases = [
            ("plain name\twith tab", "plain name\twith tab"),
            ("\u{1b}]52;c;ZXZpbA==\u{7}", "\\u{1b}]52;c;ZXZpbA==\\u{7}"),
            ("red\u{1b}[31m", "red\\u{1b}[31m"),
            ("line\nforged: saved", "line\\u{a}forged: saved"),
            ("cr\roverwrite", "cr\\u{d}overwrite"),
            ("csi\u{9b}2J", "csi\\u{9b}2J"),
            ("del\u{7f}", "del\\u{7f}"),
            ("evil\u{202E}fdp.exe", "evil\\u{202e}fdp.exe"),
            ("iso\u{2066}x\u{2069}", "iso\\u{2066}x\\u{2069}"),
            ("mark\u{200F}", "mark\\u{200f}"),
            ("日本語の部品 🔩", "日本語の部品 🔩"),
        ];
        for (input, want) in cases {
            let got = term_safe(input);
            assert_eq!(got, want, "{input:?}");
            assert!(
                !got.chars().any(|c| (c.is_control() && c != '\t')
                    || ('\u{202A}'..='\u{202E}').contains(&c)
                    || ('\u{2066}'..='\u{2069}').contains(&c)),
                "{got:?}"
            );
        }
        assert!(matches!(term_safe("clean"), std::borrow::Cow::Borrowed(_)));
    }

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
    fn global_options_are_taken_from_anywhere() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (g, rest) = split_globals(&args(&[
            "versions",
            "--json",
            "d1",
            "--account",
            "lab.local#bob",
        ]))
        .unwrap();
        assert!(g.json);
        assert_eq!(g.account, Some(("lab.local".into(), Some("bob".into()))));
        assert_eq!(rest, args(&["versions", "d1"]));
        let (g, _) = split_globals(&args(&["--account=https://x:8640/p", "whoami"])).unwrap();
        assert_eq!(g.account, Some(("https://x:8640/p".into(), None)));
        assert!(!g.json);
        assert!(split_globals(&args(&["whoami", "--account"])).is_none());
        assert!(split_globals(&args(&["--account=", "whoami"])).is_none());
        assert!(split_globals(&args(&["--account=https://x#", "whoami"])).is_none());
        assert!(is_global_option("--account=x") && !is_global_option("--yes"));
    }

    #[test]
    fn confirmation_and_share_lines() {
        assert!(is_yes("y\n") && is_yes(" YES ") && !is_yes("\n") && !is_yes("no"));
        let mut l = proto::ShareLink {
            id: "s1".into(),
            document: "d1".into(),
            version: None,
            created_by: "ada".into(),
            created_at: 0,
            expires_at: Some(86_400),
            allow_download: true,
        };
        assert_eq!(
            share_line(&l),
            "s1\tlatest\texpires 1970-01-02 00:00 UTC\tdownload\tby ada"
        );
        l.version = Some(3);
        l.expires_at = None;
        l.allow_download = false;
        assert_eq!(share_line(&l), "s1\tv3\tno expiry\tview only\tby ada");
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
