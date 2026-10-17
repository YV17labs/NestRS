//! [`Embedded`] — a folder `#[derive(Embed)]` compiles into the binary, named
//! as the source in code — and [`EmbeddedFiles`], its table, built once at
//! boot so a request is one lookup.

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use poem::http::HeaderValue;
use rust_embed::{EmbeddedFile, RustEmbed};

use crate::asset::{Asset, AssetBody, Found};
use crate::config::StaticFilesConfig;
use crate::error::StaticFilesError;
use crate::files::{Files, INDEX};
use crate::media_type::{is_html, media_type};
use crate::request_path::RequestPath;
use crate::validators::Validators;

/// A folder compiled into the binary, served in place of a directory:
/// `StaticFilesModule::for_root(Embedded::of::<WebAssets>())`.
#[derive(Clone, Copy)]
pub struct Embedded {
    name: &'static str,
    files: fn() -> EmbeddedFiles,
}

impl Embedded {
    /// The folder `E` embeds — a type deriving [`Embed`](crate::Embed).
    pub fn of<E: RustEmbed>() -> Self {
        Self {
            name: std::any::type_name::<E>(),
            files: EmbeddedFiles::of::<E>,
        }
    }

    /// Its table — or the boot's refusal when a directory is configured too,
    /// since a source is one or the other.
    pub(crate) fn files(self, config: &StaticFilesConfig) -> Result<Files, StaticFilesError> {
        if config.root.is_some() {
            return Err(StaticFilesError::RootBesideEmbedded {
                embedded: self.name,
            });
        }
        Ok(Files::Embedded((self.files)()))
    }
}

impl std::fmt::Debug for Embedded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Embedded").field(&self.name).finish()
    }
}

/// Every file of an embedded folder by its path, a directory's index under the
/// directory's own, each with the fields it answers with.
pub(crate) struct EmbeddedFiles(HashMap<String, Entry>);

#[derive(Clone)]
struct Entry {
    bytes: Bytes,
    content_type: HeaderValue,
    html: bool,
    validators: Option<Validators>,
}

impl EmbeddedFiles {
    fn of<E: RustEmbed>() -> Self {
        let mut files = HashMap::new();
        for name in E::iter() {
            let Some(file) = E::get(&name) else {
                continue;
            };
            let entry = Entry::new(&name, file);
            if let Some(directory) = directory_of_index(&name) {
                files.insert(directory.to_owned(), entry.clone());
            }
            files.insert(name.into_owned(), entry);
        }
        Self(files)
    }

    /// The file at `path`, or a directory's index — or, when `fallback` and
    /// nothing matches, the root's.
    pub(crate) fn find(&self, path: &RequestPath<'_>, fallback: bool) -> Found {
        match self.0.get(&path.key()) {
            Some(entry) => Found::Asset(entry.asset()),
            None if fallback => self
                .0
                .get("")
                .map_or(Found::Missing, |index| Found::Fallback(index.asset())),
            None => Found::Missing,
        }
    }
}

impl Entry {
    fn new(name: &str, file: EmbeddedFile) -> Self {
        let media_type = media_type(name);
        let modified = file
            .metadata
            .last_modified()
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
        Self {
            bytes: match file.data {
                std::borrow::Cow::Borrowed(bytes) => Bytes::from_static(bytes),
                std::borrow::Cow::Owned(bytes) => Bytes::from(bytes),
            },
            content_type: HeaderValue::from_static(media_type),
            html: is_html(media_type),
            validators: Validators::of_digest(file.metadata.sha256_hash(), modified),
        }
    }

    fn asset(&self) -> Asset {
        Asset {
            len: self.bytes.len() as u64,
            body: AssetBody::Bytes(self.bytes.clone()),
            content_type: self.content_type.clone(),
            html: self.html,
            validators: self.validators.clone(),
        }
    }
}

/// The directory `name` is the index of: `docs/index.html` → `docs`,
/// `index.html` → the root.
fn directory_of_index(name: &str) -> Option<&str> {
    match name.strip_suffix(INDEX)? {
        "" => Some(""),
        directory => directory.strip_suffix('/'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_answers_for_its_directory() {
        assert_eq!(directory_of_index("index.html"), Some(""));
        assert_eq!(directory_of_index("docs/index.html"), Some("docs"));
        assert_eq!(directory_of_index("docs/myindex.html"), None);
        assert_eq!(directory_of_index("app.js"), None);
    }
}
