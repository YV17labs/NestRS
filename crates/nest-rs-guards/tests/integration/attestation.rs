//! Every guard that checks an edge declares it — scanned over the repository's
//! own source, because the compiler cannot ask this everywhere.
//!
//! The four marker traits are bound at the four decorator sites: `#[controller]`
//! / `#[routes]` / `#[crud]` and a `#[gateway]` struct require [`HttpGuard`],
//! `#[resolver]` / `#[operations]` require `GraphqlGuard`, `#[messages]` require
//! `WsGuard`, `#[mcp]` / `#[tools]` require `McpGuard`. **`use_guards_global`
//! requires nothing**, deliberately: a global guard legitimately serves whichever
//! edges it implements, and bounding it on one would refuse a guard written for
//! another. That is a recorded decision, and this is its cost — a guard reached
//! only through the pool can override a `check_*` and declare nothing, and no
//! build anywhere will say so.
//!
//! What that cost bought, before this test: 22 guards across the workspace
//! overriding `check_http` with no `HttpGuard` beside it, including fixtures
//! written *while closing a hole of exactly this shape*. None was in shipped
//! code — the compiler-enforced sites cover all of those — so the drift lived
//! entirely where nothing was watching.
//!
//! Reading the sources is the only way to ask the question, and there is
//! precedent: `nest-rs-macro-hygiene`'s `emissions` test scans every `*-macros`
//! crate for a path rooted outside the framework, for the same reason — a rule
//! the compiler cannot express at every site is enforced by reading the tree.
//! This test lives here because `nest-rs-guards` owns `Guard` and the four
//! markers, and a check belongs to the crate that owns what it validates.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `check_*` method → the marker that attests it.
const EDGES: [(&str, &str); 4] = [
    ("check_http", "HttpGuard"),
    ("check_graphql", "GraphqlGuard"),
    ("check_ws_message", "WsGuard"),
    ("check_mcp", "McpGuard"),
];

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("nest-rs-guards sits two levels under the workspace root")
        .to_path_buf()
}

/// Every `.rs` file this scan may draw a conclusion from, with comments and
/// string literals blanked out.
///
/// Two directories are excluded at the source, so the two loops below cannot
/// disagree about which files exist: `target/`, which holds generated trybuild
/// workspaces — copies of the fixtures, which would double every finding — and
/// `diagnostics/`, whose fixtures are *meant* to omit the marker, that being the
/// shape the snapshot beside each proves the compiler refuses. Excluding the
/// latter here rather than only where guards are checked is what stops a marker
/// written inside a fixture from attesting a real guard elsewhere in its crate.
fn sources(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in ["crates", "demo"] {
        collect(&root.join(dir), &mut out);
    }
    assert!(
        out.len() >= 200,
        "the scan found {} sources — it is reading the wrong tree, and a scan that \
         reads nothing reports everything as clean",
        out.len()
    );
    out
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "diagnostics")
            {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(src) = std::fs::read_to_string(&path)
        {
            out.push((path, strip_noise(&src)));
        }
    }
}

/// Blank out what is not code: a comment quoting `impl Guard for X {}` — and
/// several do, that shape being what the markers exist to refuse — otherwise
/// reads as a declaration and reports a guard that does not exist, and a
/// *string* quoting `impl HttpGuard for {Self} {}` — which every
/// `on_unimplemented` note does — otherwise attests one.
///
/// A character walk rather than a truncation at the first `//`, and the
/// difference is not theoretical: `"https://…"` would cut the line under it,
/// taking any brace after it with it and unbalancing the block scan below. This
/// is `nest-rs-macro-hygiene`'s `strip_noise`, which reads every `*-macros`
/// source for the same kind of rule; blanking rather than deleting keeps byte
/// offsets, so what is found sits where it was written.
fn strip_noise(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let (mut in_string, mut in_comment, mut escaped) = (false, false, false);
    while let Some(c) = chars.next() {
        if in_comment {
            in_comment = c != '\n';
            out.push(if c == '\n' { c } else { ' ' });
        } else if in_string {
            out.push(if c == '\n' { c } else { ' ' });
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '/' && chars.peek() == Some(&'/') {
            in_comment = true;
            out.push(' ');
        } else if c == '"' {
            in_string = true;
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// The crate a file belongs to — markers are declared per crate, not per file.
fn crate_of(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .ok()
        .and_then(|rel| {
            let mut parts = rel.components();
            let top = parts.next()?;
            let name = parts.next()?;
            Some(format!(
                "{}/{}",
                top.as_os_str().display(),
                name.as_os_str().display()
            ))
        })
        .unwrap_or_default()
}

/// The body of every `impl Guard for <Name> { … }` block, by name.
///
/// Read backwards from `Guard for` rather than forwards from `impl Guard for`,
/// because the two guards at the centre of the framework are **generic** —
/// `impl<S: Strategy> Guard for AuthnGuard<S>`,
/// `impl<F: AbilityFactory> Guard for AbilityGuard<F>` — and a scan anchored on
/// the literal looked at neither.
fn guard_impls(src: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    for (at, _) in src.match_indices("Guard for ") {
        // A longer trait ending in the same word — `McpGuard for`,
        // `GraphqlOperationGuard for`. None of them is the trait this asks about.
        if src[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
        {
            continue;
        }
        // What precedes an impl header is `impl` or `impl<…>`; anything else is
        // a bound (`T: Guard`) or a where clause.
        let head = src[..at].trim_end();
        let is_header = head.ends_with("impl")
            || (head.ends_with('>')
                && head
                    .rfind("impl")
                    .is_some_and(|i| head[i + 4..].starts_with('<')));
        if !is_header {
            continue;
        }
        let after = &src[at + "Guard for ".len()..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // The forwarding blanket. It overrides all four `check_*` to delegate,
        // declares nothing itself, and is never what a decorator binds — the
        // bound is emitted against the path the developer wrote.
        if name.is_empty() || name == "Arc" {
            continue;
        }
        let Some(open) = after.find('{') else {
            continue;
        };
        out.push((name, block(&after[open..])));
    }
    out
}

/// The braced block starting at `src[0]`, counted rather than matched on a
/// column-zero `}`: an `impl` nested in a `mod tests` closes indented, so the
/// column-zero form ran a guard's body to the end of the module and credited it
/// with its siblings' `check_*`. Sound because strings and comments are blanked.
fn block(src: &str) -> &str {
    let mut depth = 0_usize;
    for (at, c) in src.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[..=at];
                }
            }
            _ => {}
        }
    }
    src
}

#[test]
fn every_guard_declares_the_edges_it_checks() {
    let root = workspace_root();
    let sources = sources(&root);

    // Marker declarations, indexed per (crate, marker): a guard and its
    // attestation may legitimately sit in different files of one crate.
    let mut declared: HashMap<(String, &str), HashSet<String>> = HashMap::new();
    for (file, src) in &sources {
        let krate = crate_of(&root, file);
        for (_, marker) in EDGES {
            for (at, _) in src.match_indices(marker) {
                let tail = src[at + marker.len()..].trim_start();
                if let Some(rest) = tail.strip_prefix("for ") {
                    let name: String = rest
                        .trim_start()
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        declared
                            .entry((krate.clone(), marker))
                            .or_default()
                            .insert(name);
                    }
                }
            }
        }
    }

    let mut missing: Vec<String> = Vec::new();
    let mut scanned = 0_usize;
    for (file, src) in &sources {
        let krate = crate_of(&root, file);
        for (name, body) in guard_impls(src) {
            scanned += 1;
            for (check, marker) in EDGES {
                let overrides = body
                    .match_indices(check)
                    .any(|(at, _)| body[..at].trim_end().ends_with("fn"));
                if !overrides {
                    continue;
                }
                let here = declared
                    .get(&(krate.clone(), marker))
                    .is_some_and(|names| names.contains(&name));
                if !here {
                    let rel = file.strip_prefix(&root).unwrap_or(file);
                    missing.push(format!(
                        "  {name} overrides `{check}` but declares no `{marker}` — {}",
                        rel.display()
                    ));
                }
            }
        }
    }

    assert!(
        scanned >= 20,
        "the scan matched {scanned} `impl Guard for` blocks — it has stopped \
         recognising the shape it reads, and a scan that finds no guards reports \
         every guard as attested"
    );

    missing.sort();
    assert!(
        missing.is_empty(),
        "a guard checks an edge it does not declare:\n{}\n\n\
         Write `impl {{Edge}}Guard for {{Guard}} {{}}` beside the `check_*` it attests. \
         The four decorator sites require it and the compiler says so there; \
         `use_guards_global` requires nothing, which is where this drifts and why \
         this test reads the sources instead of trusting a build to notice.",
        missing.join("\n"),
    );
}
