//! Standard output and error that tolerate a closed pipe.
//!
//! Rust ignores `SIGPIPE`, so `println!` into a pipe whose reader has gone (`rcad-cli cloud
//! versions D | head -1`) panics. Every line the CLI prints goes through [`line`] instead:
//! when the reader has gone, the process exits at once with status 141 (128 + `SIGPIPE`,
//! what a shell reports for a tool killed by the signal) and prints nothing more. Messages on
//! standard error ([`eline`]) are dropped silently if standard error is closed.
use std::io::Write;

/// Exit status after standard output's reader went away.
pub const BROKEN_PIPE_EXIT: i32 = 141;

/// Write `s` and a newline to standard output.
pub fn line(s: &str) {
    let mut out = std::io::stdout().lock();
    let result = out
        .write_all(s.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(e) = result {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(BROKEN_PIPE_EXIT);
        }
        // Any other failure (disk full on a redirect, ...) is reported once and is fatal:
        // output that silently stops is worse than an error.
        eline(&format!("cannot write to standard output: {e}"));
        std::process::exit(1);
    }
}

/// Write `s` and a newline to standard error; a failure is ignored (there is nowhere left
/// to report it).
pub fn eline(s: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err
        .write_all(s.as_bytes())
        .and_then(|()| err.write_all(b"\n"));
}

/// Write `s` to standard error without a newline and flush it (a prompt).
pub fn prompt(s: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(s.as_bytes()).and_then(|()| err.flush());
}

/// `println!` through [`line`].
macro_rules! say {
    ($($arg:tt)*) => {
        $crate::out::line(&format!($($arg)*))
    };
}

/// `eprintln!` through [`eline`].
macro_rules! esay {
    ($($arg:tt)*) => {
        $crate::out::eline(&format!($($arg)*))
    };
}

pub(crate) use {esay, say};
