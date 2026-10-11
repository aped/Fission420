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
//!
//! # Deferred measurement (threaded mode)
//!
//! Exact B-rep measurement dominates regeneration of cut bodies (~1.8 s of
//! ~2.4 s for the 80×50 plate with a hole, release build). In threaded mode the
//! session defers it ([`Session::set_defer_measurements`]): a command's reply
//! carries the new geometry and `summary.measuring == true`, and a separate
//! measurement thread measures the newest body (latest job wins; jobs for
//! superseded revisions are dropped before they start). The result is fed back
//! to the session through the worker, which then sends a *follow-up* reply
//! ([`Reply::followup`], command [`Command::Measure`]) tagged with the document
//! id and revision it describes. Follow-ups are not answers to submitted
//! commands (they don't count against [`Engine::busy`]). The UI applies one
//! only if it still shows that document revision, so a late measurement can
//! never be presented as describing newer geometry.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex};
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

/// One visible body inside a [`DisplayScene`].
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayBody {
    pub id: f420_document::BodyId,
    /// Triangle-mesh body (STL import), not an exact B-rep.
    pub mesh_body: bool,
    /// Display face groups of this body in the combined mesh.
    pub groups: std::ops::Range<u32>,
    /// Viewport tint palette index (see `Viewport::set_group_tints`).
    pub tint: u8,
}

/// Combined display tessellation of every visible body. Group ids are offset
/// per body so they never collide; hidden bodies are absent (not drawn, not
/// pickable, not part of Fit).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DisplayScene {
    pub mesh: RenderMesh,
    pub bodies: Vec<DisplayBody>,
    /// Visible bodies that could not be tessellated ("name: error"); they are
    /// left out of the scene so the other bodies still display.
    pub skipped: Vec<String>,
}

impl DisplayScene {
    /// Body owning display group `g`.
    pub fn body_of(&self, g: u32) -> Option<&DisplayBody> {
        self.bodies.iter().find(|b| b.groups.contains(&g))
    }

    /// Tint table for the viewport.
    pub fn tints(&self) -> Vec<(std::ops::Range<u32>, u8)> {
        self.bodies
            .iter()
            .map(|b| (b.groups.clone(), b.tint))
            .collect()
    }
}

/// Mesh-body tint palette index.
pub const MESH_TINT: u8 = 7;

/// What the display mesh should become after a command.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshUpdate {
    /// Geometry did not change.
    Unchanged,
    /// New tessellation of all visible bodies (empty mesh = no visible body).
    Replace(DisplayScene),
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
    /// Deferred-measurement follow-up (not the answer to a submitted command):
    /// `measurements` describe `summary.doc_id` / `summary.revision`.
    pub followup: bool,
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
        followup: false,
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
        followup: false,
    }
}

fn display_mesh(session: &Session) -> MeshUpdate {
    let Some(m) = session.model() else {
        return MeshUpdate::Replace(DisplayScene::default());
    };
    match display_scene(m.evaluation()) {
        Ok(s) => MeshUpdate::Replace(s),
        Err(e) => MeshUpdate::Failed(e),
    }
}

/// Tessellate every visible body into one mesh with per-body group ranges.
/// A body that fails to tessellate is listed in `skipped`, not drawn.
/// B-rep bodies get one group per B-rep face; a mesh body is one group.
/// Tints follow the body's position in creation order (stable under
/// visibility changes); mesh bodies use [`MESH_TINT`].
pub fn display_scene(ev: &f420_document::Evaluation) -> Result<DisplayScene, String> {
    use f420_document::BodyGeometry;
    let mut scene = DisplayScene::default();
    let mut next_group = 0u32;
    for (i, b) in ev.bodies.iter().enumerate() {
        if !b.visible {
            continue;
        }
        let (tm, mesh_body) = match &b.geometry {
            BodyGeometry::Brep(body) => {
                match f420_kernel::tessellate_display(body, DISPLAY_CHORD) {
                    Ok(tm) => (tm, false),
                    Err(e) => {
                        scene.skipped.push(format!("{}: {e}", b.name));
                        continue;
                    }
                }
            }
            BodyGeometry::Mesh(mb) => (mb.mesh.clone(), true),
        };
        let rm = to_render_mesh(&tm);
        let groups = if mesh_body {
            1
        } else {
            rm.tri_group.iter().max().map_or(0, |g| g.saturating_add(1))
        };
        let base = u32::try_from(scene.mesh.positions.len())
            .map_err(|_| "display mesh too large".to_string())?;
        scene.mesh.positions.extend(&rm.positions);
        if rm.normals.len() == rm.positions.len() {
            scene.mesh.normals.extend(&rm.normals);
        } else {
            // Keep normals aligned with positions (flat shading fallback).
            scene.mesh.normals.clear();
        }
        scene
            .mesh
            .indices
            .extend(rm.indices.iter().map(|i| i + base));
        scene.mesh.tri_group.extend(
            rm.tri_group
                .iter()
                .map(|g| next_group + if mesh_body { 0 } else { *g }),
        );
        let end = next_group.saturating_add(groups.max(1));
        scene.bodies.push(DisplayBody {
            id: b.id,
            mesh_body,
            groups: next_group..end,
            tint: if mesh_body {
                MESH_TINT
            } else {
                (i % 6) as u8 + 1
            },
        });
        next_group = end;
    }
    if scene.mesh.normals.len() != scene.mesh.positions.len() {
        scene.mesh.normals.clear();
    }
    Ok(scene)
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

/// Input of the session worker: one channel, so a blocking receive wakes on
/// commands and on measurement results alike.
enum WorkerIn {
    Commands(Vec<Command>),
    Measured(Measured),
    /// Sent by [`Engine`]'s `Drop` (the measurer holds a sender clone, so channel
    /// disconnection alone cannot end the worker).
    Shutdown,
}

/// A body to measure, tagged with the state it belongs to.
struct MeasureJob {
    doc_id: u64,
    revision: u64,
    bodies: Vec<(f420_document::BodyId, f420_kernel::Body)>,
}

/// Latest-job-wins slot shared with the measurement thread.
#[derive(Default)]
struct MeasureSlot {
    job: Option<MeasureJob>,
    shutdown: bool,
}

/// (doc id, revision, bodies measured, failures (body, error)). Bodies are
/// measured independently so one failure does not withhold the others.
type Measured = (
    u64,
    u64,
    Vec<(f420_document::BodyId, Measurements)>,
    Vec<(f420_document::BodyId, String)>,
);

/// `F420_TRACE_ENGINE=1`: log the deferred-measurement pipeline to stderr.
fn trace(msg: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("F420_TRACE_ENGINE").is_some_and(|v| v == "1")) {
        eprintln!("{:.3} engine: {}", trace_clock(), msg());
    }
}

/// Seconds since the first trace call (shared by engine and UI traces).
pub fn trace_clock() -> f64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn measurer(slot: Arc<(Mutex<MeasureSlot>, Condvar)>, out: Sender<WorkerIn>) {
    let (lock, cv) = &*slot;
    loop {
        let job = {
            let Ok(mut g) = lock.lock() else { return };
            loop {
                if g.shutdown {
                    return;
                }
                if let Some(j) = g.job.take() {
                    break j;
                }
                g = match cv.wait(g) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
        };
        trace(|| format!("measure start doc {} rev {}", job.doc_id, job.revision));
        let t0 = Instant::now();
        let (mut ok, mut failed) = (Vec::new(), Vec::new());
        for (id, b) in &job.bodies {
            match f420_kernel::measure(b) {
                Ok(m) => ok.push((*id, m)),
                Err(e) => failed.push((*id, e.to_string())),
            }
        }
        trace(|| {
            format!(
                "measure done doc {} rev {} in {:.0} ms ok={}",
                job.doc_id,
                job.revision,
                t0.elapsed().as_secs_f64() * 1e3,
                failed.is_empty()
            )
        });
        if out
            .send(WorkerIn::Measured((job.doc_id, job.revision, ok, failed)))
            .is_err()
        {
            return;
        }
    }
}

/// Follow-up reply for a finished measurement, or `None` if it is stale.
fn adopt(session: &mut Session, (doc_id, revision, ok, failed): Measured) -> Option<Reply> {
    // Bodies that measured are kept (and cached) even when another failed.
    let adopted = !ok.is_empty() && session.adopt_measurements(doc_id, revision, ok);
    let response = if failed.is_empty() {
        if !adopted {
            trace(|| format!("stale measurement doc {doc_id} rev {revision} dropped"));
            return None; // stale: the document moved on
        }
        trace(|| format!("adopted doc {doc_id} rev {revision}"));
        Response {
            ok: true,
            command: "measure".into(),
            message: Some(format!("measured revision {revision}")),
            ..Default::default()
        }
    } else {
        let text = failed
            .iter()
            .map(|(id, e)| format!("body {id}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        // Each failing body gets a stable failed state: reported once here,
        // not retried until its geometry changes.
        if !session.fail_body_measurements(doc_id, revision, failed) && !adopted {
            return None; // stale failure: nothing to report
        }
        Response {
            ok: false,
            command: "measure".into(),
            error: Some(f420_document::DocError::Validation(format!(
                "measurement of revision {revision} failed: {text}"
            ))),
            ..Default::default()
        }
    };
    Some(Reply {
        command: Command::Measure,
        response,
        summary: session.summary(),
        measurements: session
            .model()
            .and_then(|m| m.evaluation().measurements.clone()),
        mesh: MeshUpdate::Unchanged,
        followup: true,
    })
}

/// Worker loop: FIFO over submitted batches, draining everything already queued
/// before each command so [`handle_next`] can see what follows it. Deferred
/// measurements are computed on a helper thread and adopted between commands.
fn worker(
    rx: Receiver<WorkerIn>,
    self_tx: Sender<WorkerIn>,
    tx: Sender<Reply>,
    repaint: Option<Box<dyn Fn() + Send>>,
) {
    let mut session = Session::new();
    session.set_defer_measurements(true);
    let slot = Arc::new((Mutex::new(MeasureSlot::default()), Condvar::new()));
    let measurer_handle = {
        let slot = slot.clone();
        std::thread::Builder::new()
            .name("f420-measure".into())
            .spawn(move || measurer(slot, self_tx))
            .ok()
    };
    let notify = || {
        if let Some(r) = &repaint {
            r();
        }
    };
    let mut queue: VecDeque<Command> = VecDeque::new();
    let mut stopping = false;
    // Last (doc_id, revision) handed to the measurement thread.
    let mut last_job: Option<(u64, u64)> = None;
    'run: loop {
        if stopping && queue.is_empty() {
            break; // Shutdown received and every queued command answered
        }
        // Block only when there is nothing queued; then drain without blocking.
        let mut inbox = Vec::new();
        if queue.is_empty() {
            match rx.recv() {
                Ok(m) => inbox.push(m),
                Err(_) => break,
            }
        }
        while let Ok(m) = rx.try_recv() {
            inbox.push(m);
        }
        for m in inbox {
            match m {
                WorkerIn::Commands(batch) => queue.extend(batch),
                WorkerIn::Measured(done) => {
                    if let Some(r) = adopt(&mut session, done) {
                        if tx.send(r).is_err() {
                            break 'run;
                        }
                        notify();
                    }
                }
                WorkerIn::Shutdown => stopping = true,
            }
        }
        let Some(reply) = handle_next(&mut session, &mut queue) else {
            continue;
        };
        if tx.send(reply).is_err() {
            break;
        }
        notify();
        // Queue the newest unmeasured body (replacing any older pending job).
        // Compare the key before deep-copying the body for the measurer.
        let key = session
            .model()
            .filter(|m| m.measurements_pending())
            .map(|m| (session.summary().doc_id, m.revision));
        let pending = match key {
            Some(k) if last_job != Some(k) => session.pending_measurement(),
            _ => None,
        };
        if let Some((doc_id, revision, bodies)) = pending {
            last_job = Some((doc_id, revision));
            trace(|| format!("queue measure doc {doc_id} rev {revision}"));
            let (lock, cv) = &*slot;
            if let Ok(mut g) = lock.lock() {
                g.job = Some(MeasureJob {
                    doc_id,
                    revision,
                    bodies,
                });
                cv.notify_one();
            }
        }
    }
    let (lock, cv) = &*slot;
    if let Ok(mut g) = lock.lock() {
        g.shutdown = true;
        g.job = None;
        cv.notify_one();
    }
    if let Some(h) = measurer_handle {
        let _ = h.join();
    }
}

enum Backend {
    Sync {
        session: Box<Session>,
        ready: VecDeque<Reply>,
    },
    Threaded {
        tx: Option<Sender<WorkerIn>>,
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
                let (tx, worker_rx) = channel::<WorkerIn>();
                let (worker_tx, rx) = channel::<Reply>();
                let self_tx = tx.clone();
                let handle = std::thread::Builder::new()
                    .name("f420-session".into())
                    .spawn(move || worker(worker_rx, self_tx, worker_tx, repaint))
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
                let sent = handle.is_some()
                    && tx
                        .as_ref()
                        .is_some_and(|t| t.send(WorkerIn::Commands(cmds)).is_ok());
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
                    if !r.followup {
                        self.in_flight = self.in_flight.saturating_sub(1);
                    }
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
            // The worker answers everything already queued, then exits (the
            // measurement thread holds a sender, so closing alone is not enough).
            if let Some(t) = tx.take() {
                let _ = t.send(WorkerIn::Shutdown);
            }
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

    /// Review run 3 #17: one body failing to measure does not withhold the
    /// others' measurements.
    #[test]
    fn a_failed_body_measurement_keeps_the_others() {
        let mut s = Session::new();
        s.set_defer_measurements(true);
        s.execute(&Command::NewDocument);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../f420-kernel/tests/fixtures/step/two_boxes_compound_mm.step");
        let r = s.execute(&Command::ImportStep {
            path,
            units_to_mm: None,
        });
        assert!(r.ok, "{:?}", r.error);
        let (doc, rev, bodies) = s.pending_measurement().expect("pending");
        assert_eq!(bodies.len(), 2);
        let (good, bad) = (bodies[0].0, bodies[1].0);
        let m = f420_kernel::measure(&bodies[0].1).unwrap();
        let reply = adopt(
            &mut s,
            (
                doc,
                rev,
                vec![(good, m)],
                vec![(bad, "injected".to_string())],
            ),
        )
        .expect("current revision: reported");
        assert!(!reply.response.ok);
        fn by(s: &Session, id: f420_document::BodyId) -> f420_commands::BodySummary {
            s.summary().bodies.into_iter().find(|b| b.id == id).unwrap()
        }
        assert!(by(&s, good).measurements.is_some());
        assert!(by(&s, bad).measurements.is_none());
        assert_eq!(by(&s, bad).measure_error.as_deref(), Some("injected"));
        assert!(
            !s.model().unwrap().measurements_pending(),
            "not waited on forever"
        );
        // Run 4: an unrelated edit does not retry the failed body (no new
        // job, no banner per revision); its failed state stays visible.
        let r = s.execute(&Command::RenameBody {
            body: good,
            name: "renamed".into(),
        });
        assert!(r.ok);
        assert!(!s.model().unwrap().measurements_pending());
        assert!(s.pending_measurement().is_none());
        assert_eq!(by(&s, bad).measure_error.as_deref(), Some("injected"));
        assert!(by(&s, good).measurements.is_some());
    }

    thread_local! {
        /// Deferred-measurement follow-ups seen while collecting command replies.
        static FOLLOWUPS: std::cell::RefCell<Vec<Reply>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Wait (bounded) for exactly `n` command replies (deferred-measurement
    /// follow-ups are not answers to commands; they are stashed for
    /// [`followup_for`]).
    fn collect(e: &mut Engine, n: usize) -> Vec<Reply> {
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut out = Vec::new();
        while out.len() < n {
            match e.poll() {
                Some(r) if r.followup => FOLLOWUPS.with(|f| f.borrow_mut().push(r)),
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
        while let Some(r) = e.poll() {
            assert!(r.followup, "no extra command replies");
            FOLLOWUPS.with(|f| f.borrow_mut().push(r));
        }
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
            Command::extrude(None, S1, 10.0),
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

    /// Wait (bounded) for the deferred-measurement follow-up of `revision`;
    /// follow-ups for other (earlier) revisions are skipped.
    fn followup_for(e: &mut Engine, revision: u64) -> Reply {
        let stashed = FOLLOWUPS.with(|f| {
            let mut f = f.borrow_mut();
            let i = f.iter().position(|r| r.summary.revision == revision)?;
            Some(f.remove(i))
        });
        if let Some(r) = stashed {
            return r;
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match e.poll() {
                Some(r) if r.followup && r.summary.revision == revision => return r,
                Some(r) if r.followup => {
                    assert!(r.summary.revision < revision, "follow-up from the future");
                }
                Some(r) => panic!("unexpected command reply {:?}", r.command),
                None => {
                    assert!(Instant::now() < deadline, "no measurement follow-up");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    fn volume(m: &Option<Measurements>) -> f64 {
        match m.as_ref().map(|m| &m.volume) {
            Some(f420_kernel::VolumeResult::Value { volume, .. }) => *volume,
            other => panic!("no volume: {other:?}"),
        }
    }

    /// Threaded mode defers B-rep measurement: the command reply carries new
    /// geometry with `summary.measuring`, the follow-up carries the volume for
    /// exactly that revision; a measurement overtaken by a newer edit is never
    /// delivered.
    #[test]
    fn deferred_measurement_follows_geometry_and_is_never_stale() {
        let mut e = Engine::new(ExecMode::Threaded, None);
        let base = ready(&mut e);
        assert!(e.submit(width_edit(100.0)));
        let r = collect(&mut e, 1).remove(0);
        assert!(r.response.ok);
        assert!(
            matches!(r.mesh, MeshUpdate::Replace(_)),
            "geometry is in the reply"
        );
        assert!(r.summary.measuring && r.measurements.is_none());
        let f = followup_for(&mut e, r.summary.revision);
        assert_eq!(f.summary.doc_id, r.summary.doc_id);
        assert!(!f.summary.measuring);
        assert!((volume(&f.measurements) - 100.0 * 50.0 * 10.0).abs() < 1e-6);
        // Two edits back to back (not coalesced): follow-ups only ever describe
        // the revision they name, and the newest revision gets measured.
        assert!(e.submit_all(vec![width_edit(90.0), Command::Inspect, width_edit(95.0)]));
        let rs = collect(&mut e, 3);
        let last = rs.last().unwrap().summary.revision;
        assert!(last > base.revision);
        let f = followup_for(&mut e, last);
        assert!((volume(&f.measurements) - 95.0 * 50.0 * 10.0).abs() < 1e-6);
        // Undo twice, back to the width-100 state measured above: its
        // measurements come from the cache in the reply itself (nothing pending).
        assert!(e.submit_all(vec![Command::Undo, Command::Undo]));
        let u = collect(&mut e, 2).remove(1);
        assert!(!u.summary.measuring);
        assert!((volume(&u.measurements) - 100.0 * 50.0 * 10.0).abs() < 1e-6);
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
        assert_eq!(r[3].mesh, MeshUpdate::Replace(DisplayScene::default()));
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

    /// Timing harness for docs/behavior/timings.md (not a pass/fail test):
    /// `cargo test --release -p f420-app --lib engine::tests::timings -- --ignored --nocapture`
    /// Round trip = submit() to poll() returning the reply, through the real
    /// threaded engine (worker execute + display tessellation + channel).
    #[test]
    #[ignore = "timing harness; run explicitly"]
    fn timings() {
        const RUNS: usize = 7;
        fn stats(name: &str, mut v: Vec<f64>) {
            v.sort_by(f64::total_cmp);
            println!(
                "TIMING {name}: median {:.1} ms, min {:.1}, max {:.1} (n={})",
                v[v.len() / 2],
                v[0],
                v[v.len() - 1],
                v.len()
            );
        }
        fn round_trip(e: &mut Engine, c: Command) -> (Reply, f64) {
            let t = Instant::now();
            assert!(e.submit(c));
            let r = collect(e, 1).remove(0);
            assert!(r.response.ok, "{:?}", r.response.error);
            (r, t.elapsed().as_secs_f64() * 1000.0)
        }
        let plate = |w: f64| {
            let mut c = setup();
            c[1] = Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: w,
                    height: 50.0,
                    center: None,
                },
            };
            c.push(Command::CreateSketch {
                name: None,
                plane: f420_commands::end_cap_of(FeatureId(2)),
                template: SketchTemplate::CenteredCircle {
                    diameter: 10.0,
                    center: None,
                },
            });
            c.push(Command::CutThroughAll {
                name: None,
                sketch: FeatureId(3),
            });
            c
        };
        let mut e = Engine::new(ExecMode::Threaded, None);
        assert!(e.submit_all(plate(80.0)));
        assert!(collect(&mut e, 5).iter().all(|r| r.response.ok));
        // SetParameter width on the plate with a cut: regeneration (sketch solve,
        // extrude, boolean cut, B-rep volume) + display tessellation. Values never
        // repeat, so the prefix cache cannot serve the cut.
        let (mut rt, mut worker, mut measured) = (vec![], vec![], vec![]);
        for i in 0..RUNS {
            let t = Instant::now();
            let (r, ms) = round_trip(&mut e, width_edit(90.0 + i as f64));
            rt.push(ms);
            worker.push(r.elapsed().as_secs_f64() * 1000.0);
            let f = followup_for(&mut e, r.summary.revision);
            assert!(f.response.ok && f.measurements.is_some());
            measured.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        stats(
            "plate SetParameter width round trip (geometry; measurement deferred)",
            rt,
        );
        stats("plate SetParameter width worker time", worker);
        stats(
            "plate SetParameter width until measurements arrive",
            measured,
        );
        // Undo of the same: served from the regeneration cache.
        let (mut rt, mut worker) = (vec![], vec![]);
        for _ in 0..RUNS.min(5) {
            let (r, ms) = round_trip(&mut e, Command::Undo);
            rt.push(ms);
            worker.push(r.elapsed().as_secs_f64() * 1000.0);
        }
        stats("plate Undo round trip (cached prefix)", rt);
        stats("plate Undo worker time", worker);
        // Full plate regeneration from scratch through the session only (no
        // tessellation): the cost of Open / from_document with a cut.
        let mut cold = vec![];
        for i in 0..RUNS {
            let mut s = Session::new();
            let cmds = plate(80.0 + i as f64);
            let t = Instant::now();
            for c in &cmds {
                assert!(s.execute(c).ok);
            }
            cold.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        stats(
            "plate full build via Session (5 commands, no tessellation)",
            cold,
        );
        // SetParameter on a body without booleans (box): baseline round trip.
        let mut e2 = Engine::new(ExecMode::Threaded, None);
        assert!(e2.submit_all(setup()));
        collect(&mut e2, 3);
        let mut rt = vec![];
        for i in 0..RUNS {
            rt.push(round_trip(&mut e2, width_edit(60.0 + i as f64)).1);
        }
        stats("box SetParameter width round trip (no boolean)", rt);
        // Scenario B extrude (5-vertex editor loop).
        let (mut rt, mut worker) = (vec![], vec![]);
        for _ in 0..RUNS {
            let mut e3 = Engine::new(ExecMode::Threaded, None);
            let steps = f420_commands::scenarios::scenario_b_workflow(
                std::path::Path::new("/nonexistent/unused.f420.json"),
                std::path::Path::new("/nonexistent"),
            )
            .unwrap_or_else(|err| panic!("{err}"));
            assert!(e3.submit_all(steps[..2].iter().map(|s| s.cmd.clone()).collect()));
            collect(&mut e3, 2);
            let (r, ms) = round_trip(&mut e3, steps[2].cmd.clone());
            rt.push(ms);
            worker.push(r.elapsed().as_secs_f64() * 1000.0);
        }
        stats("scenario B extrude round trip", rt);
        stats("scenario B extrude worker time", worker);
    }
}
