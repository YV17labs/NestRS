//! [`DiskRoot`] — the directory a module serves at runtime, and the
//! confinement every file is held to: resolved through every link, and under
//! the root it was at boot.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use poem::http::HeaderValue;

use crate::asset::{Asset, AssetBody, Found};
use crate::config::StaticFilesConfig;
use crate::error::StaticFilesError;
use crate::files::{Files, INDEX};
use crate::media_type::{is_html, media_type};
use crate::request_path::{RequestPath, hides};
use crate::validators::Validators;

/// A file up to this size is read whole while it is looked up; a larger one
/// is streamed in chunks of this size.
pub(crate) const CHUNK: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct DiskRoot {
    root: Arc<Path>,
}

impl DiskRoot {
    /// The directory `config` names, resolved — or the boot's refusal: unset,
    /// missing, or not a directory.
    pub(crate) fn files(config: &StaticFilesConfig) -> Result<Files, StaticFilesError> {
        let root = config.root.as_deref().ok_or(StaticFilesError::RootUnset)?;
        let root = std::fs::canonicalize(root)
            .map_err(|source| StaticFilesError::RootUnresolved { source })?;
        if !root.is_dir() {
            return Err(StaticFilesError::RootNotADirectory);
        }
        Ok(Files::Disk(Self { root: root.into() }))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// What answers at `path`, the fallback's index included, in one blocking
    /// call off the executor.
    pub(crate) async fn find(&self, path: &RequestPath<'_>, fallback: bool) -> io::Result<Found> {
        let root = Arc::clone(&self.root);
        let candidate = root.join(path.relative());
        // `?` takes tokio's conversion, which says a panic without its payload.
        tokio::task::spawn_blocking(move || match resolve(&root, &candidate)? {
            Found::Missing if fallback => Ok(match resolve(&root, &root)? {
                Found::Asset(index) => Found::Fallback(index),
                other => other,
            }),
            found => Ok(found),
        })
        .await?
    }
}

/// A candidate path, opened and held to the root.
enum Opened {
    File {
        path: PathBuf,
        file: File,
        len: u64,
        modified: Option<SystemTime>,
    },
    Directory(PathBuf),
    /// Not there, or neither a regular file nor a directory.
    Missing,
    Escaped,
}

/// What `candidate` resolves to under `root`: a regular file, a directory's
/// index, or nothing — never a listing.
fn resolve(root: &Path, candidate: &Path) -> io::Result<Found> {
    let opened = match open(root, candidate)? {
        Opened::Directory(path) => open(root, &path.join(INDEX))?,
        opened => opened,
    };
    match opened {
        Opened::File {
            path,
            file,
            len,
            modified,
        } => asset(&path, file, len, modified),
        Opened::Directory(_) | Opened::Missing => Ok(Found::Missing),
        Opened::Escaped => Ok(Found::Escaped),
    }
}

/// `candidate` with every link resolved, opened if it stays under `root` on a
/// name the root serves; what it is, the open handle's own metadata says.
fn open(root: &Path, candidate: &Path) -> io::Result<Opened> {
    let Some(canonical) = or_missing(std::fs::canonicalize(candidate))? else {
        return Ok(Opened::Missing);
    };
    match canonical.strip_prefix(root) {
        Ok(relative) if !hides(relative) => {}
        _ => return Ok(Opened::Escaped),
    }
    let Some(file) = or_missing(read_only().open(&canonical))? else {
        return Ok(Opened::Missing);
    };
    let metadata = file.metadata()?;
    Ok(if metadata.is_file() {
        Opened::File {
            path: canonical,
            file,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    } else if metadata.is_dir() {
        Opened::Directory(canonical)
    } else {
        Opened::Missing
    })
}

#[cfg(unix)]
fn read_only() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;

    // Opening a FIFO blocks until a writer appears; non-blocking, it opens and
    // its metadata refuses it. A regular file reads the same either way.
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NONBLOCK);
    options
}

#[cfg(not(unix))]
fn read_only() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options
}

/// The regular file at `path`, read whole when small, its length and time
/// read off the open handle.
fn asset(path: &Path, mut file: File, len: u64, modified: Option<SystemTime>) -> io::Result<Found> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let media_type = media_type(&name);
    let (body, len) = match usize::try_from(len) {
        Ok(small) if small <= CHUNK => {
            let mut bytes = Vec::with_capacity(small);
            file.read_to_end(&mut bytes)?;
            let len = bytes.len() as u64;
            (AssetBody::Bytes(Bytes::from(bytes)), len)
        }
        _ => (AssetBody::File(file), len),
    };
    Ok(Found::Asset(Asset {
        body,
        len,
        content_type: HeaderValue::from_static(media_type),
        html: is_html(media_type),
        validators: Validators::of_file(len, modified),
    }))
}

/// `result`, or `None` for an error a request's own path causes — a name that
/// is not there, a file where a directory was expected, a name too long — as
/// against one the operator must hear about.
fn or_missing<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::NotADirectory
                    | io::ErrorKind::InvalidFilename
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}
