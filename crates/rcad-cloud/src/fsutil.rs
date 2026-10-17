//! Atomic file replacement shared by the account file and the cache.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary path next to `path` (same directory, so the final rename stays on one
/// filesystem), unique per process and call.
pub(crate) fn temp_path_for(path: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}-{n}", std::process::id()))
}

/// Create `path`'s parent directories.
pub(crate) fn ensure_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => std::fs::create_dir_all(dir),
        _ => Ok(()),
    }
}

/// Open a new temporary file; `private` creates it with mode 0600 on Unix before any byte
/// is written, so a secret is never readable by other users, even briefly.
pub(crate) fn create_temp(path: &Path, private: bool) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    opts.open(path)
}

/// Replace `path` with `bytes` atomically: write a temporary file in the same directory,
/// flush it to disk, then rename it over `path`. On any error `path` is left unchanged.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    ensure_parent(path)?;
    let tmp = temp_path_for(path);
    let result = (|| {
        let mut f = create_temp(&tmp, private)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
