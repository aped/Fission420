//! Command execution engine: owns the authoritative [`Session`].
//!
//! In [`ExecMode::Threaded`] the session lives on a worker thread; the UI sends
//! [`Command`]s over a channel and polls [`Reply`]s (response, document summary,
//! measurements, and a freshly tessellated display mesh), so multi-second
//! regenerations never block the UI thread. [`ExecMode::Sync`] runs the same code
//! inline (used by UI tests for determinism).

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

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

/// Execute one command and derive everything the UI needs from the session.
pub fn execute(session: &mut Session, cmd: Command) -> Reply {
    let before = session.summary();
    let response = session.execute(&cmd);
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

enum Backend {
    Sync {
        session: Box<Session>,
        ready: VecDeque<Reply>,
    },
    Threaded {
        tx: Option<Sender<Command>>,
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
                let (tx, worker_rx) = channel::<Command>();
                let (worker_tx, rx) = channel::<Reply>();
                let handle = std::thread::Builder::new()
                    .name("f420-session".into())
                    .spawn(move || {
                        let mut session = Session::new();
                        while let Ok(cmd) = worker_rx.recv() {
                            let reply = execute(&mut session, cmd);
                            if worker_tx.send(reply).is_err() {
                                break;
                            }
                            if let Some(r) = &repaint {
                                r();
                            }
                        }
                    })
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
        match &mut self.backend {
            Backend::Sync { session, ready } => {
                ready.push_back(execute(session, cmd));
                true
            }
            Backend::Threaded { tx, handle, .. } => {
                let sent = handle.is_some() && tx.as_ref().is_some_and(|t| t.send(cmd).is_ok());
                if sent {
                    self.in_flight += 1;
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
            tx.take(); // closes the channel; worker loop exits after current command
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}
