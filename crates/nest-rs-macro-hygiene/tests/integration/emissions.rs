//! The testable form `framework.md` states for macro hygiene, executed.
//!
//! > a `*-macros` crate emits only `::std`/`::core` paths or paths routed
//! > through its surface crate's re-exports (`::nest_rs_<x>::<dep>`) — never a
//! > bare third-party path (`::anyhow`, `::tracing`, …), which resolves against
//! > the *consumer's* extern prelude and breaks any app lacking that direct dep.
//!
//! The rule was written as checkable and never checked. `#[crud]` emitted
//! `::uuid::Uuid` for three routes and one resolver argument, so a controller
//! whose own source never wrote `uuid` failed to compile with `E0433` blamed on
//! the attribute. It survived because the compile witness next door cannot
//! reach `#[crud]`, and because the generator's e2e happens to add `uuid` for
//! an unrelated reason (`g resource` bootstraps `g auth`, whose claims type
//! names it).
//!
//! So this scan is deliberately **not** a list of banned crate names: it allows
//! the framework's own roots and rejects everything else. A decorator added
//! next year reaching for a crate nobody thought to ban fails here on the day
//! it is written.

use std::fs;
use std::path::{Path, PathBuf};

/// Path roots an expansion may name. Everything else resolves against the
/// consumer's extern prelude, which is theirs to populate and not ours to
/// assume.
///
/// `crate`/`self`/`Self`/`super` are not roots after `::`, but a `::crate`
/// typo would be a compile error in the macro crate long before this test.
const ALLOWED_ROOTS: &[&str] = &["std", "core", "alloc"];

/// The framework's own crates, matched by prefix: `::nest_rs_http`,
/// `::nest_rs_resource`, and the umbrella `::nest_rs` a call site may see after
/// [`reroot`](https://docs.rs/nest-rs-codegen).
fn is_framework_root(ident: &str) -> bool {
    ident == "nest_rs" || ident.starts_with("nest_rs_")
}

/// Every `crates/nest-rs-*-macros/src/**/*.rs` in the workspace.
fn macro_sources() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in crates/")
        .to_path_buf();
    let mut out = Vec::new();
    for entry in fs::read_dir(&crates).expect("crates/ is readable") {
        let dir = entry.expect("a readable entry").path();
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if !name.ends_with("-macros") {
            continue;
        }
        collect_rs(&dir.join("src"), &mut out);
    }
    assert!(
        out.len() >= 10,
        "the scan found {} macro sources — it is reading the wrong directory",
        out.len()
    );
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Blank out what is not an emission: line comments carry the *prose about*
/// these paths (every `crate = ` override is explained beside the code that
/// sets it), and string literals carry diagnostics that quote paths at the
/// developer. Both would report a violation that does not exist.
///
/// Blanking rather than deleting keeps byte offsets, so a reported line number
/// still points at the real line.
fn strip_noise(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
                out.push(c);
            } else {
                out.push(' ');
            }
            continue;
        }
        if in_string {
            out.push(if c == '\n' { c } else { ' ' });
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            in_line_comment = true;
            out.push(' ');
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

/// Every `::krate::` that opens a multi-segment path.
///
/// Three conditions, and each one is load-bearing against a shape this scan
/// otherwise misreads — `reroot`'s own walk records the same trap, having tried
/// and abandoned inspecting what precedes a `::`:
///
/// * the `::` opens a path (not preceded by an identifier character or colon),
///   so `nest_rs_ws::nest_rs_http::X` reports one root rather than two;
/// * the segment is snake_case and does not start with `_`, so `<#ty>::PATH`
///   and the `__nestrs_*` seams a decorator calls on the developer's own type
///   are not mistaken for crates;
/// * a second `::` follows, so `<#ty>::apply` — a method on an interpolated
///   receiver, where the `>` before `::` looks exactly like `<T as Tr>::assoc`
///   — is not mistaken for one either.
///
/// A crate named in an expansion is always `::krate::Item`, so nothing real is
/// lost; what is dropped is only the shapes a text scan cannot resolve.
fn rooted_paths(src: &str) -> Vec<(usize, String)> {
    let bytes = src.as_bytes();
    let mut hits = Vec::new();
    let mut line = 1usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            let opens_path = i == 0 || {
                let prev = bytes[i - 1];
                !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b':')
            };
            if opens_path {
                let mut j = i + 2;
                let start = j;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let segment = &src[start..j];
                let looks_like_a_crate = j > start
                    && !segment.starts_with('_')
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    && src[j..].starts_with("::");
                if looks_like_a_crate {
                    hits.push((line, segment.to_owned()));
                }
                i = j;
                continue;
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    hits
}

#[test]
fn no_decorator_emits_a_path_the_consumer_must_declare() {
    let mut violations = Vec::new();
    for file in macro_sources() {
        let src = fs::read_to_string(&file).expect("a readable macro source");
        for (line, root) in rooted_paths(&strip_noise(&src)) {
            if ALLOWED_ROOTS.contains(&root.as_str()) || is_framework_root(&root) {
                continue;
            }
            violations.push(format!("{}:{line} emits `::{root}`", file.display()));
        }
    }
    assert!(
        violations.is_empty(),
        "a decorator's expansion names a crate the use site never wrote, so its \
         manifest has to declare one — see *The umbrella is the front door*. \
         Route the path through the owning surface crate's re-export \
         (`::nest_rs_<x>::<dep>`):\n  {}",
        violations.join("\n  "),
    );
}

#[test]
fn the_scan_would_catch_a_bare_path() {
    let src = r#"
        // ::anyhow in a comment is prose, not an emission
        let msg = "::tracing in a literal is a diagnostic, not an emission";
        quote! { ::anyhow::anyhow!("boom") }
    "#;
    let roots: Vec<String> = rooted_paths(&strip_noise(src))
        .into_iter()
        .map(|(_, r)| r)
        .collect();
    assert_eq!(
        roots,
        vec!["anyhow".to_owned()],
        "only the emission counts — comments and literals are stripped"
    );
}
