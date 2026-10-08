//! [`Material`] — a variable's value as the deployment gave it, inline or through
//! the file its `<KEY>_FILE` names, and [`read_material`], the one bounded reader
//! for such a file.

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The most a `<KEY>_FILE` is read for — a bound on a path naming `/dev/zero` or
/// a log file by mistake.
pub(crate) const MAX_MATERIAL_BYTES: u64 = 1024 * 1024;

/// The bytes read for one key through
/// [`ConfigService::material`](crate::ConfigService::material), and the file they
/// came from when the deployment named a path — what a reloading consumer
/// watches. `Debug` shows the path and the length, never the bytes.
#[derive(Clone)]
pub struct Material {
    /// The material, as read.
    pub bytes: Vec<u8>,
    /// The file the bytes were read from, as the variable named it with
    /// surrounding whitespace trimmed; `None` when they were inline.
    pub path: Option<PathBuf>,
}

impl fmt::Debug for Material {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Material")
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .field("path", &self.path)
            .finish()
    }
}

/// Read `path` if it is a regular file (symlinks followed — a Kubernetes secret
/// mount is one) of at most a mebibyte: the one reader for a value a deployment
/// names by path.
///
/// Opening a FIFO blocks forever, so on unix the file is opened **non-blocking**
/// and its kind checked on the handle, closing the look-then-open race; other
/// platforms only narrow it, checking the path before opening and the handle after.
pub fn read_material(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?
    };
    #[cfg(not(unix))]
    let file = {
        if !std::fs::metadata(path)?.is_file() {
            return Err(not_a_regular_file());
        }
        std::fs::File::open(path)?
    };
    if !file.metadata()?.is_file() {
        return Err(not_a_regular_file());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MATERIAL_BYTES + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MATERIAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!(
                "the file is larger than {MAX_MATERIAL_BYTES} bytes, which no configuration value is"
            ),
        ));
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
#[expect(
    clippy::let_underscore_must_use,
    reason = "the tests tear down temp files and a watchdog thread best-effort"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    /// A path flipped between a regular file and a FIFO while it is read: a
    /// check made before opening can be passed by the file and the open then
    /// land on the pipe, which blocks forever. The watchdog turns a hang into a
    /// failure instead of a stuck suite.
    #[test]
    fn a_path_swapped_to_a_fifo_never_blocks_the_read() {
        let dir = std::env::temp_dir().join(format!("nest-rs-config-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let regular = dir.join("regular.pem");
        std::fs::write(&regular, "material").expect("write");
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "a FIFO to swap in");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&regular, &link).expect("symlink");

        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let (stop, dir, link) = (Arc::clone(&stop), dir.clone(), link.clone());
            std::thread::spawn(move || {
                let staged = dir.join("link.staged");
                while !stop.load(Ordering::Relaxed) {
                    for target in [&fifo, &regular] {
                        let _ = std::fs::remove_file(&staged);
                        let _ = std::os::unix::fs::symlink(target, &staged);
                        let _ = std::fs::rename(&staged, &link);
                    }
                }
            })
        };
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let link = link.clone();
            std::thread::spawn(move || {
                for _ in 0..2_000 {
                    if let Ok(bytes) = read_material(&link) {
                        assert_eq!(bytes, b"material", "only the regular file is ever read");
                    }
                }
                let _ = tx.send(());
            });
        }
        let outcome = rx.recv_timeout(Duration::from_secs(20));
        stop.store(true, Ordering::Relaxed);
        let _ = swapper.join();
        let _ = std::fs::remove_dir_all(&dir);
        outcome.expect("every read returns while the path is swapped to a FIFO");
    }
}

fn not_a_regular_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "not a regular file — a directory, a device or a pipe cannot hold the material",
    )
}
