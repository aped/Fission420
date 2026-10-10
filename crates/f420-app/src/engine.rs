//! Command execution engine: owns the authoritative [`Session`].
//!
//! In [`ExecMode::Threaded`] the session lives on a worker thread; the UI sends
//! [`Command`]s over a channel and polls [`Reply`]s (response, document summary,
//! measurements, and a freshly tessellated display mesh), so multi-second
//! regenerations never block the UI thread. [`ExecMode::Sync`] runs the same code
//! inline (used by UI tests for determinism).
//!
//! # Ordering, coalescing and stale results
//!
//! * One worker owns the one [`Session`] and handles commands strictly in
//!   submission order. Every submitted command gets exactly one [`Reply`], and
//!   replies arrive in submission order (the app counts in-flight commands).
//! * Coalescing: before handling a command the worker drains everything already
//!   queued. A `SetParameter` that is immediately followed in the queue by another
//!   `SetParameter` of the same `(feature, name)` is *superseded*: it is not
//!   executed (no regeneration, no history entry) and its reply has
//!   `response.ok == true`, `response.superseded == true` ([`Reply::superseded`]),
//!   the current (unchanged) summary, and [`MeshUpdate::Unchanged`]. Only adjacent
//!   same-key edits coalesce, so the end state equals executing every command in
//!   order: the last value wins, and an Undo after a burst undoes the burst as one
//!   step because only its last edit committed. [`Engine::submit_all`] queues
//!   several commands atomically, which makes coalescing inside that batch
//!   deterministic; with single [`Engine::submit`]s it depends on timing.
//! * Stale results: a reply describes the session *after* its command, and every
//!   [`DocSummary`] carries `doc_id` (bumped by New/Open/Close) and `revision`
//!   (bumped by every commit, undo and redo). Because the worker is FIFO and
//!   single-threaded, a reply never describes a state older than an earlier reply;
//!   the UI applies replies in arrival order and can drop any reply whose `doc_id`
//!   differs from the document it currently shows. A running kernel operation is
//!   never interrupted; its reply is simply followed by the replies of the
//!   commands queued after it.
//! * Profiling: `response.elapsed_ms` ([`Reply::elapsed`]) is the worker time for
//!   the command including display tessellation.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use f420_commands::{Command, DocSummary, Response, Session};
use f420_kernel::Measurements;
use f420_viewport::RenderMesh;

/// Display tessellation chord tolerance (mm). Display only; STL export uses
/// the chord the user requests.
pub const DISPLAY_CHORD: f64 = 0.05;

// Compile-time proof that the document can move to a worker thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Session>();
    assert_send::<Command>();
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    Threaded,
    Sync,
}

/// What the display mesh should become after a command.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshUpdate {
    /// Geometry did not change.
    Unchanged,
    /// New body tessellation (empty mesh = no body).
    Replace(RenderMesh),
    /// Body exists but display tessellation failed.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub command: Command,
    pub response: Response,
    /// Always the post-command document state (also on failure).
    pub summary: DocSummary,
    pub measurements: Option<Measurements>,
    pub mesh: MeshUpdate,
}

impl Reply {
    /// The command was skipped because a newer edit of the same parameter was
    /// queued right behind it (see the module docs). Nothing changed.
    pub fn superseded(&self) -> bool {
        self.response.superseded
    }

    /// Worker time spent on the command, including display tessellation
    /// (zero for superseded commands).
    pub fn elapsed(&self) -> Duration {
        self.response
            .elapsed_ms
            .filter(|ms| ms.is_finite() && *ms >= 0.0)
            .map(|ms| Duration::from_secs_f64(ms / 1000.0))
            .unwrap_or_default()
    }
}

/// `Some("F1.width")` when `cmd` is made redundant by `next`: both set the same
/// parameter of the same feature, so only `next`'s value can be observed.
fn supersede_key(cmd: &Command, next: Option<&Command>) -> Option<String> {
    match (cmd, next?) {
        (
            Command::SetParameter { feature, name, .. },
            Command::SetParameter {
                feature: f2,
                name: n2,
                ..
            },
        ) if feature == f2 && name == n2 => Some(format!("{feature}.{name}")),
        _ => None,
    }
}

/// Reply for a command that is skipped (not executed) because it was superseded.
fn superseded_reply(session: &Session, cmd: Command, key: &str) -> Reply {
    let feature = match &cmd {
        Command::SetParameter { feature, .. } => Some(*feature),
        _ => None,
    };
    let response = Response {
        ok: true,
        command: cmd.name().into(),
        feature,
        message: Some(format!("superseded by newer edit of {key}")),
        superseded: true,
        elapsed_ms: Some(0.0),
        ..Default::default()
    };
    Reply {
        command: cmd,
        response,
        summary: session.summary(),
        measurements: session
            .model()
            .and_then(|m| m.evaluation().measurements.clone()),
        mesh: MeshUpdate::Unchanged,
    }
}

/// Handle the front of `queue`: execute it, or answer it as superseded when the
/// next queued command makes it redundant. `None` when the queue is empty.
fn handle_next(session: &mut Session, queue: &mut VecDeque<Command>) -> Option<Reply> {
    let cmd = queue.pop_front()?;
    Some(match supersede_key(&cmd, queue.front()) {
        Some(key) => superseded_reply(session, cmd, &key),
        None => execute(session, cmd),
    })
}

/// Execute one command and derive everything the UI needs from the session.
pub fn execute(session: &mut Session, cmd: Command) -> Reply {
    let t0 = Instant::now();
    let before = session.summary();
    let mut response = session.execute(&cmd);
    let summary = session.summary();
    let geometry_changed = response.ok
        && (before.revision != summary.revision
            || before.open != summary.open
            || matches!(
                cmd,
                Command::NewDocument | Command::Open { .. } | Command::Close
            ));
    let mesh = if geometry_changed {
        display_mesh(session)
    } else {
        MeshUpdate::Unchanged
    };
    let measurements = session
        .model()
        .and_then(|m| m.evaluation().measurements.clone());
    response.elapsed_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
    Reply {
        command: cmd,
        response,
        summary,
        measurements,
        mesh,
    }
}

fn display_mesh(session: &Session) -> MeshUpdate {
    let Some(body) = session.model().and_then(|m| m.evaluation().body.as_ref()) else {
        return MeshUpdate::Replace(RenderMesh::default());
    };
    match f420_kernel::tessellate_display(body, DISPLAY_CHORD) {
        Ok(tm) => MeshUpdate::Replace(to_render_mesh(&tm)),
        Err(e) => MeshUpdate::Failed(e.to_string()),
    }
}

/// Convert the kernel's f64 display tessellation to the viewport's f32 mesh.
/// Groups are B-rep face ids (display only, never persisted as references).
pub fn to_render_mesh(tm: &f420_kernel::TriMesh) -> RenderMesh {
    let f = |p: &[f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32];
    RenderMesh {
        positions: tm.positions.iter().map(f).collect(),
        normals: if tm.normals.len() == tm.positions.len() {
            tm.normals.iter().map(f).collect()
        } else {
            Vec::new()
        },
        indices: tm.triangles.iter().flatten().copied().collect(),
        tri_group: tm.tri_face.clone(),
    }
}

/// Worker loop: FIFO over submitted batches, draining everything already queued
/// before each command so [`handle_next`] can see what follows it.
fn worker(rx: Receiver<Vec<Command>>, tx: Sender<Reply>, repaint: Option<Box<dyn Fn() + Send>>) {
    let mut session = Session::new();
    let mut queue: VecDeque<Command> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(batch) => queue.extend(batch),
                Err(_) => break, // engine dropped and the queue is drained
            }
        }
        while let Ok(batch) = rx.try_recv() {
            queue.extend(batch);
        }
        let Some(reply) = handle_next(&mut session, &mut queue) else {
            continue;
        };
        if tx.send(reply).is_err() {
            break;
        }
        if let Some(r) = &repaint {
            r();
        }
    }
}

enum Backend {
    Sync {
        session: Box<Session>,
        ready: VecDeque<Reply>,
    },
    Threaded {
        tx: Option<Sender<Vec<Command>>>,
        rx: Receiver<Reply>,
        handle: Option<JoinHandle<()>>,
    },
}

/// Front-end handle to the session. Not `Clone`: one owner (the app).
pub struct Engine {
    backend: Backend,
    in_flight: usize,
    worker_lost: bool,
}

impl Engine {
    /// `repaint` is called from the worker after each reply (wakes the UI).
    pub fn new(mode: ExecMode, repaint: Option<Box<dyn Fn() + Send>>) -> Self {
        let backend = match mode {
            ExecMode::Sync => Backend::Sync {
                session: Box::new(Session::new()),
                ready: VecDeque::new(),
            },
            ExecMode::Threaded => {
                let (tx, worker_rx) = channel::<Vec<Command>>();
                let (worker_tx, rx) = channel::<Reply>();
                let handle = std::thread::Builder::new()
                    .name("f420-session".into())
                    .spawn(move || worker(worker_rx, worker_tx, repaint))
                    .ok();
                Backend::Threaded {
                    tx: Some(tx),
                    rx,
                    handle,
                }
            }
        };
        Self {
            backend,
            in_flight: 0,
            worker_lost: false,
        }
    }

    pub fn mode(&self) -> ExecMode {
        match self.backend {
            Backend::Sync { .. } => ExecMode::Sync,
            Backend::Threaded { .. } => ExecMode::Threaded,
        }
    }

    /// Queue a command. Returns false if the worker is gone.
    pub fn submit(&mut self, cmd: Command) -> bool {
        self.submit_all(vec![cmd])
    }

    /// Queue several commands atomically, in order: the worker sees them all at
    /// once, so adjacent same-parameter edits inside the batch always coalesce
    /// (see the module docs). One reply per command, in order. In Sync mode the
    /// batch is handled immediately with the same rule. Returns false (and
    /// queues nothing) if the worker is gone.
    pub fn submit_all(&mut self, cmds: Vec<Command>) -> bool {
        if cmds.is_empty() {
            return true;
        }
        let n = cmds.len();
        match &mut self.backend {
            Backend::Sync { session, ready } => {
                let mut queue: VecDeque<Command> = cmds.into();
                while let Some(r) = handle_next(session, &mut queue) {
                    ready.push_back(r);
                }
                true
            }
            Backend::Threaded { tx, handle, .. } => {
                let sent = handle.is_some() && tx.as_ref().is_some_and(|t| t.send(cmds).is_ok());
                if sent {
                    self.in_flight += n;
                } else {
                    self.worker_lost = true;
                }
                sent
            }
        }
    }

    /// Non-blocking: next finished reply, if any.
    pub fn poll(&mut self) -> Option<Reply> {
        match &mut self.backend {
            Backend::Sync { ready, .. } => ready.pop_front(),
            Backend::Threaded { rx, .. } => match rx.try_recv() {
                Ok(r) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    Some(r)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.in_flight = 0;
                    self.worker_lost = true;
                    None
                }
            },
        }
    }

    /// Commands submitted but not yet answered.
    pub fn busy(&self) -> bool {
        self.in_flight > 0
    }

    /// The worker thread died (e.g. a panic escaped the kernel guard).
    pub fn worker_lost(&self) -> bool {
        self.worker_lost
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Backend::Threaded { tx, handle, .. } = &mut self.backend {
            tx.take(); // closes the channel; the worker finishes its queue, then exits
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f420_commands::SketchTemplate;
    use f420_document::{FeatureId, SketchPlane};

    const S1: FeatureId = FeatureId(1);

    /// Wait (bounded) for exactly `n` replies.
    fn collect(e: &mut Engine, n: usize) -> Vec<Reply> {
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut out = Vec::new();
        while out.len() < n {
            match e.poll() {
                Some(r) => out.push(r),
                None => {
                    assert!(!e.worker_lost(), "worker lost");
                    assert!(
                        Instant::now() < deadline,
                        "timed out after {} replies",
                        out.len()
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        assert!(!e.busy(), "all submitted commands answered");
        assert!(e.poll().is_none(), "no extra replies");
        out
    }

    fn setup() -> Vec<Command> {
        vec![
            Command::NewDocument,
            Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: 80.0,
                    height: 50.0,
                    center: None,
                },
            },
            Command::Extrude {
                name: None,
                sketch: S1,
                distance: 10.0,
            },
        ]
    }

    fn width_edit(v: f64) -> Command {
        Command::SetParameter {
            feature: S1,
            name: "width".into(),
            value: v,
        }
    }

    fn width(summary: &DocSummary) -> Option<f64> {
        summary
            .features
            .iter()
            .find(|f| f.id == S1)?
            .parameters
            .iter()
            .find(|(n, _)| n == "width")
            .map(|(_, v)| *v)
    }

    fn ready(e: &mut Engine) -> DocSummary {
        let n = setup().len();
        assert!(e.submit_all(setup()));
        let r = collect(e, n);
        assert!(r.iter().all(|r| r.response.ok), "setup failed");
        r.last()
            .map(|r| r.summary.clone())
            .unwrap_or_else(|| panic!())
    }

    /// Five rapid width edits and an Undo, queued atomically: the first four are
    /// superseded, the fifth commits, Undo returns to the original width.
    #[test]
    fn burst_of_edits_coalesces_and_undo_reverts_the_burst() {
        for mode in [ExecMode::Threaded, ExecMode::Sync] {
            let mut e = Engine::new(mode, None);
            let base = ready(&mut e);
            let mut cmds: Vec<Command> = (1..=5).map(|i| width_edit(80.0 + i as f64)).collect();
            cmds.push(Command::Undo);
            assert!(e.submit_all(cmds.clone()));
            let r = collect(&mut e, cmds.len());
            for (reply, cmd) in r.iter().zip(&cmds) {
                assert_eq!(&reply.command, cmd, "{mode:?}: replies in submission order");
                assert!(reply.response.ok, "{mode:?}: {:?}", reply.response.error);
            }
            for s in &r[..4] {
                assert!(s.superseded(), "{mode:?}");
                assert_eq!(s.mesh, MeshUpdate::Unchanged);
                assert_eq!(s.summary.revision, base.revision, "nothing committed");
                assert_eq!(width(&s.summary), Some(80.0));
                assert_eq!(s.elapsed(), Duration::ZERO);
            }
            let last_edit = &r[4];
            assert!(!last_edit.superseded());
            assert_eq!(width(&last_edit.summary), Some(85.0));
            assert_eq!(last_edit.summary.revision, base.revision + 1);
            assert!(matches!(last_edit.mesh, MeshUpdate::Replace(_)));
            assert!(last_edit.elapsed() > Duration::ZERO);
            let undo = &r[5];
            assert!(!undo.superseded());
            assert_eq!(
                width(&undo.summary),
                Some(80.0),
                "{mode:?}: undo reverts the burst"
            );
            assert_eq!(undo.summary.revision, base.revision + 2);
            assert_eq!(undo.summary.can_undo.as_deref(), Some("Extrude"));
        }
    }

    /// Different parameters, or an edit followed by a different command, never
    /// coalesce.
    #[test]
    fn only_adjacent_same_parameter_edits_coalesce() {
        let mut e = Engine::new(ExecMode::Threaded, None);
        let base = ready(&mut e);
        let cmds = vec![
            width_edit(90.0),
            Command::SetParameter {
                feature: S1,
                name: "height".into(),
                value: 40.0,
            },
            width_edit(95.0),
            Command::Inspect,
            width_edit(96.0),
            width_edit(97.0),
        ];
        assert!(e.submit_all(cmds.clone()));
        let r = collect(&mut e, cmds.len());
        let sup: Vec<bool> = r.iter().map(Reply::superseded).collect();
        assert_eq!(sup, vec![false, false, false, false, true, false]);
        let last = &r[5].summary;
        assert_eq!(width(last), Some(97.0));
        assert_eq!(last.revision, base.revision + 4);
    }

    /// Individually submitted rapid edits: coalescing depends on timing, but the
    /// invariants do not: one reply per command in order, the edit right before
    /// Undo is never superseded, and Undo reverts to the value before the last
    /// *committed* edit.
    #[test]
    fn rapid_individual_submits_keep_fifo_and_consistent_end_state() {
        let mut e = Engine::new(ExecMode::Threaded, None);
        ready(&mut e);
        let mut cmds: Vec<Command> = (1..=5).map(|i| width_edit(80.0 + i as f64)).collect();
        cmds.push(Command::Undo);
        for c in &cmds {
            assert!(e.submit(c.clone()));
        }
        let r = collect(&mut e, cmds.len());
        for (reply, cmd) in r.iter().zip(&cmds) {
            assert_eq!(&reply.command, cmd);
            assert!(reply.response.ok);
        }
        assert!(!r[4].superseded());
        let committed: Vec<f64> = r[..5]
            .iter()
            .filter(|x| !x.superseded())
            .filter_map(|x| width(&x.summary))
            .collect();
        assert_eq!(committed.last(), Some(&85.0));
        let expected_after_undo = if committed.len() >= 2 {
            committed[committed.len() - 2]
        } else {
            80.0
        };
        assert_eq!(width(&r[5].summary), Some(expected_after_undo));
        // Revisions (bumped by commits and by undo) never go backwards.
        assert!(
            r.windows(2)
                .all(|w| w[0].summary.revision <= w[1].summary.revision)
        );
    }

    /// NewDocument behind pending edits: the edits' replies belong to the old
    /// document; the final reply carries a new doc_id and an empty history, so
    /// the UI can tell late results of the old document from the new one.
    #[test]
    fn new_document_after_pending_edits_changes_doc_id() {
        let mut e = Engine::new(ExecMode::Threaded, None);
        let base = ready(&mut e);
        let cmds = vec![
            width_edit(90.0),
            width_edit(95.0),
            width_edit(70.0),
            Command::NewDocument,
        ];
        for c in &cmds {
            assert!(e.submit(c.clone()));
        }
        let r = collect(&mut e, cmds.len());
        for x in &r[..3] {
            assert_eq!(x.summary.doc_id, base.doc_id);
        }
        let last = &r[3].summary;
        assert_ne!(last.doc_id, base.doc_id);
        assert!(last.open && last.features.is_empty());
        assert_eq!(last.revision, 0);
        assert_eq!(r[3].mesh, MeshUpdate::Replace(RenderMesh::default()));
    }

    #[test]
    fn empty_batch_and_superseded_reply_shape() {
        let mut e = Engine::new(ExecMode::Sync, None);
        assert!(e.submit_all(vec![]));
        assert!(e.poll().is_none());
        ready(&mut e);
        assert!(e.submit_all(vec![width_edit(82.0), width_edit(83.0)]));
        let r = collect(&mut e, 2);
        assert_eq!(r[0].response.command, "set_parameter");
        assert_eq!(r[0].response.feature, Some(S1));
        assert_eq!(
            r[0].response.message.as_deref(),
            Some("superseded by newer edit of F1.width")
        );
        assert!(r[0].response.error.is_none());
        assert!(!r[1].superseded() && r[1].response.ok);
    }
}
