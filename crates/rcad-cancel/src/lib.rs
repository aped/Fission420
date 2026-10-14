//! Cooperative cancellation shared by every long-running stage (file readers in
//! `rcad-exchange`, STEP import in `rcad-kernel`, regeneration in
//! `rcad-document`).
//!
//! One process-wide flag (there is one document worker per process): the UI
//! thread [`request`]s cancellation, the worker [`clear`]s it before each
//! command, and loops poll it through [`Poll`] (an atomic load every `n`
//! items, so the cost is negligible). A stage that sees the request stops and
//! returns its own "cancelled" error; nothing is committed by the caller.
use std::sync::atomic::{AtomicBool, Ordering};

static FLAG: AtomicBool = AtomicBool::new(false);

/// Ask the running stage to stop at its next check.
pub fn request() {
    FLAG.store(true, Ordering::SeqCst);
}

/// Clear a request (before a new command starts).
pub fn clear() {
    FLAG.store(false, Ordering::SeqCst);
}

/// Whether cancellation was requested.
pub fn requested() -> bool {
    FLAG.load(Ordering::SeqCst)
}

/// Checks the flag on the first call of [`Poll::tick`] and then every `every`
/// calls (so even a short loop checks once).
#[derive(Debug, Clone)]
pub struct Poll {
    every: u32,
    left: u32,
}

impl Poll {
    pub fn new(every: u32) -> Poll {
        let every = every.max(1);
        Poll { every, left: 1 }
    }

    /// True when cancellation was requested (checked every `every` ticks).
    pub fn tick(&mut self) -> bool {
        self.left -= 1;
        if self.left == 0 {
            self.left = self.every;
            requested()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_checks_every_n_ticks() {
        clear();
        let mut p = Poll::new(3);
        request();
        assert_eq!(
            [p.tick(), p.tick(), p.tick(), p.tick()],
            [true, false, false, true]
        );
        clear();
        assert!(!p.tick() && !p.tick() && !p.tick());
    }
}
