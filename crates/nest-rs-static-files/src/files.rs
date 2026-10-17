//! [`Files`] — the source a module serves, as the boot resolved it: a
//! directory read at runtime, or a folder the binary carries.

use crate::asset::Found;
use crate::disk::DiskRoot;
use crate::embedded::EmbeddedFiles;
use crate::request_path::RequestPath;

/// The file a directory answers with, and the one a single-page app's
/// navigation falls back to. Never a listing.
pub(crate) const INDEX: &str = "index.html";

pub(crate) enum Files {
    Disk(DiskRoot),
    Embedded(EmbeddedFiles),
}

impl Files {
    /// What answers at `path`: the file, a directory's [`INDEX`], or — when
    /// `fallback` and nothing matches — the root's index.
    pub(crate) async fn find(
        &self,
        path: &RequestPath<'_>,
        fallback: bool,
    ) -> std::io::Result<Found> {
        match self {
            Self::Disk(root) => root.find(path, fallback).await,
            Self::Embedded(files) => Ok(files.find(path, fallback)),
        }
    }
}
