//! Where a join reads its population from.
//!
//! Every member list here is walked from the tree, never listed: a family that
//! grows a member the day someone writes it is the whole point, and a literal
//! list is the defect the rule names.

use std::path::{Path, PathBuf};
use std::{fs, io};

/// The repository root, from this crate's own manifest — so a join is run from
/// anywhere and reads the same tree.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in crates/")
        .parent()
        .expect("crates/ sits at the repo root")
        .to_path_buf()
}

/// Every `.rs` under `dir`, `target/` excluded. A directory that does not exist
/// yields nothing: a join names its own floor rather than relying on this to
/// fail.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The path as the repo spells it, for a message a reader can paste into `rg`.
pub fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Compared by value, so the wrapping a `\` continuation introduces never
/// decides whether two spellings are the same string.
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A file parsed as Rust, or `None` when it is not (a fixture that must not
/// compile, a generated stub). A join reads Rust with Rust's parser: a
/// `\`-continued literal and a `#[cfg(test)]` block are the two things a text
/// scan reads wrong, and both cost a false reading here before this crate.
pub fn parsed(path: &Path) -> Option<syn::File> {
    let text = fs::read_to_string(path).ok()?;
    syn::parse_file(&text).ok()
}

/// Read a file, propagating the failure — used where absence is a bug rather
/// than a case.
pub fn read(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}
