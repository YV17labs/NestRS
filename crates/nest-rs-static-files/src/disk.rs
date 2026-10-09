//! [`DiskRoot`] — the directory a module serves at runtime, and the
//! confinement every file is held to: resolved through every link, and under
//! the root it was at boot.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        tokio::task::spawn_blocking(move || match resolve(&root, &candidate)? {
            Found::Missing if fallback => Ok(match resolve(&root, &root)? {
                Found::Asset(index) => Found::Fallback(index),
                other => other,
            }),
            found => Ok(found),
        })
        .await
        .map_err(io::Error::other)?
    }
}

/// A candidate path, opened and held to the root.
enum Opened {
    File(PathBuf, File, Metadata),
    Missing,
    Escaped,
}

/// What `candidate` resolves to under `root`: a regular file, a directory's
/// index, or nothing — never a listing.
fn resolve(root: &Path, candidate: &Path) -> io::Result<Found> {
    match open(root, candidate)? {
        Opened::File(path, _, metadata) if metadata.is_dir() => {
            match open(root, &path.join(INDEX))? {
                Opened::File(index, file, metadata) if metadata.is_file() => {
                    asset(&index, file, &metadata)
                }
                Opened::Escaped => Ok(Found::Escaped),
                Opened::File(..) | Opened::Missing => Ok(Found::Missing),
            }
        }
        Opened::File(path, file, metadata) if metadata.is_file() => asset(&path, file, &metadata),
        Opened::File(..) | Opened::Missing => Ok(Found::Missing),
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
    Ok(Opened::File(canonical, file, metadata))
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
fn asset(path: &Path, mut file: File, metadata: &Metadata) -> io::Result<Found> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let media_type = media_type(&name);
    let (body, len) = match usize::try_from(metadata.len()) {
        Ok(len) if len <= CHUNK => {
            let mut bytes = Vec::with_capacity(len);
            file.read_to_end(&mut bytes)?;
            let len = bytes.len() as u64;
            (AssetBody::Bytes(Bytes::from(bytes)), len)
        }
        _ => (AssetBody::File(file), metadata.len()),
    };
    Ok(Found::Asset(Asset {
        body,
        len,
        content_type: HeaderValue::from_static(media_type),
        html: is_html(media_type),
        validators: Validators::of_file(len, metadata.modified().ok()),
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
