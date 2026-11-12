//! The directory of the app crate `#[nest_rs::main]` expanded in, captured at
//! compile time: a development artifact the app commits — its SDL, its
//! OpenAPI document — lands beside its manifest whatever directory the process
//! runs from, and each app of a workspace writes its own.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The app crate's directory, once a `#[nest_rs::main]` ran.
static APP_DIR: OnceLock<&'static Path> = OnceLock::new();

/// Record `dir`, the `CARGO_MANIFEST_DIR` of the crate `#[nest_rs::main]`
/// expanded in. A process has one `main`, so the first recording stands.
pub(crate) fn capture(dir: &'static str) {
    APP_DIR.get_or_init(|| Path::new(dir));
}

/// The app crate's directory, once a `#[nest_rs::main]` ran: where the `.env`
/// cascade reads the app's own files.
pub fn app_dir() -> Option<&'static Path> {
    APP_DIR.get().copied()
}

/// `path` under the app crate's directory when it is relative and a
/// `#[nest_rs::main]` ran; as given otherwise — an absolute path, or a process
/// no `main` started, whose working directory then resolves it.
pub fn in_app_dir(path: &Path) -> PathBuf {
    under(app_dir(), path)
}

fn under(dir: Option<&Path>, path: &Path) -> PathBuf {
    match dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_joins_the_app_dir_and_an_absolute_one_stands() {
        let app = Path::new("/srv/acme/apps/api");
        assert_eq!(
            under(Some(app), Path::new("schema.graphql")),
            app.join("schema.graphql")
        );
        assert_eq!(
            under(Some(app), Path::new("/var/lib/acme/openapi.json")),
            Path::new("/var/lib/acme/openapi.json")
        );
        assert_eq!(
            under(None, Path::new("schema.graphql")),
            Path::new("schema.graphql")
        );
    }
}
