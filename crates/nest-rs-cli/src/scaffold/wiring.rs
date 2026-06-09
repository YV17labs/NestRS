//! Idempotent source edits — the "auto-wiring" half of the scaffolder.
//!
//! Each function returns a [`Transform`]: given a file's current text it
//! returns `Some(new_text)` when it changed something, or `None` when the
//! wiring is already present (re-running a generator is a no-op). All
//! transforms are anchored on the shapes our own templates emit, never on a
//! full Rust parse.

/// A pure, idempotent rewrite of a file's contents.
pub type Transform = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Ensure a single `mod`/`pub mod`/`use` declaration line is present.
pub fn ensure_decl(line: &str) -> Transform {
    ensure_lines(vec![line.to_string()])
}

/// Ensure several declaration lines are present in one pass — use this rather
/// than multiple `edit`s on the same file (each `edit` re-reads the file from
/// disk, so two would clobber each other). Each line is inserted after the
/// last existing line sharing its leading keyword (`mod` with `mod`, `pub use`
/// with `pub use`).
pub fn ensure_lines(new_lines: Vec<String>) -> Transform {
    let new_lines: Vec<String> = new_lines
        .into_iter()
        .map(|l| l.trim_end().to_string())
        .collect();
    Box::new(move |content: &str| {
        let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
        let mut changed = false;
        for line in &new_lines {
            if lines.iter().any(|l| l.trim() == line) {
                continue;
            }
            let group = leading_keyword(line);
            let insert_at = lines
                .iter()
                .rposition(|l| leading_keyword(l) == group)
                .map(|i| i + 1)
                .unwrap_or(lines.len());
            lines.insert(insert_at, line.clone());
            changed = true;
        }
        changed.then(|| rejoin(&lines, content))
    })
}

/// Ensure a module is imported into a `#[module(imports = [ … ])]` block:
/// inserts `use <use_path>;` among the top-of-file `use` lines and the bare
/// `<ident>,` entry just before the closing `]` of the imports array.
pub fn ensure_module_import(use_path: &str, ident: &str) -> Transform {
    let use_line = format!("use {use_path};");
    let ident = ident.to_string();
    Box::new(move |content: &str| {
        let mut changed = false;
        let mut lines: Vec<String> = content.lines().map(str::to_string).collect();

        // 1. The `use` import at the top of the file.
        if !lines.iter().any(|l| l.trim() == use_line) {
            let insert_at = lines
                .iter()
                .rposition(|l| l.trim_start().starts_with("use "))
                .map(|i| i + 1)
                .unwrap_or(0);
            lines.insert(insert_at, use_line.clone());
            changed = true;
        }

        // 2. The entry inside `imports = [ … ]`.
        if let Some((start, end)) = imports_span(&lines) {
            let already = lines[start..=end].iter().any(|l| entry_matches(l, &ident));
            if !already {
                if start == end {
                    // Single-line `imports = [A, B]` — splice into the brackets.
                    if let Some(spliced) = splice_inline(&lines[start], &ident) {
                        lines[start] = spliced;
                        changed = true;
                    }
                } else {
                    let indent = entry_indent(&lines, end);
                    lines.insert(end, format!("{indent}{ident},"));
                    changed = true;
                }
            }
        }

        changed.then(|| rejoin(&lines, content))
    })
}

/// Two grouping buckets: module declarations (`mod`/`pub mod`) sit together,
/// imports (`use`/`pub use`) sit together — so a new `pub mod` lands with the
/// `mod`s (before the `use`s), not appended at the end of the file.
fn leading_keyword(line: &str) -> &'static str {
    let t = line.trim_start();
    if t.starts_with("pub mod ") || t.starts_with("mod ") {
        "mod"
    } else if t.starts_with("pub use ") || t.starts_with("use ") {
        "use"
    } else {
        ""
    }
}

/// Line range `[start..=end]` where `start` is the line containing
/// `imports = [` and `end` is the line holding its closing `]`.
fn imports_span(lines: &[String]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|l| l.contains("imports = ["))?;
    // Single-line form: `imports = [A, B]` on one line.
    if lines[start].contains(']') {
        return Some((start, start));
    }
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with(']'))
        .map(|i| start + 1 + i)?;
    Some((start, end))
}

/// Splice `ident` before the `]` of a single-line `imports = [ … ]`.
fn splice_inline(line: &str, ident: &str) -> Option<String> {
    let open = line.find('[')?;
    let close = line[open..].find(']').map(|i| open + i)?;
    let inner = line[open + 1..close].trim();
    let new_inner = if inner.is_empty() {
        ident.to_string()
    } else {
        format!("{inner}, {ident}")
    };
    Some(format!("{}{new_inner}{}", &line[..=open], &line[close..]))
}

fn entry_matches(line: &str, ident: &str) -> bool {
    line.split([',', '[', ']', ' ', '\t'])
        .any(|tok| tok == ident)
}

/// Indentation to use for a new entry inserted before the `]` at `end`,
/// copied from the preceding entry when there is one.
fn entry_indent(lines: &[String], end: usize) -> String {
    if end > 0 && !lines[end - 1].contains("imports = [") {
        let ws: String = lines[end - 1]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        if !ws.is_empty() {
            return ws;
        }
    }
    "        ".to_string()
}

/// Re-join preserving whether the original ended with a trailing newline.
fn rejoin(lines: &[String], original: &str) -> String {
    let joined = lines.join("\n");
    if original.ends_with('\n') {
        format!("{joined}\n")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_decl_groups_and_is_idempotent() {
        let src = "mod module;\nmod service;\n\npub use module::Foo;\n";
        let t = ensure_decl("pub mod http;");
        let out = t(src).expect("inserts");
        assert!(out.contains("pub mod http;"));
        // grouped after `mod` decls, before the blank line / pub use
        let idx_mod = out.find("pub mod http;").unwrap();
        let idx_use = out.find("pub use module::Foo;").unwrap();
        assert!(idx_mod < idx_use);
        // re-running is a no-op
        assert!(ensure_decl("pub mod http;")(&out).is_none());
    }

    #[test]
    fn ensure_module_import_into_multiline_block() {
        let src = "use nest_rs_core::module;\nuse nest_rs_http::HttpModule;\n\n#[module(\n    imports = [\n        HttpModule::for_root(None),\n    ],\n)]\npub struct AppModule;\n";
        let t = ensure_module_import("features::posts::PostsHttpModule", "PostsHttpModule");
        let out = t(src).expect("inserts");
        assert!(out.contains("use features::posts::PostsHttpModule;"));
        assert!(out.contains("        PostsHttpModule,"));
        // entry sits inside the imports array, before the closing bracket
        let entry = out.find("PostsHttpModule,").unwrap();
        let close = out.find("    ],").unwrap();
        assert!(entry < close);
        // idempotent
        assert!(
            ensure_module_import("features::posts::PostsHttpModule", "PostsHttpModule")(&out)
                .is_none()
        );
    }

    #[test]
    fn ensure_module_import_single_line_block() {
        let src = "use nest_rs_http::HttpModule;\n\n#[module(imports = [HttpModule::for_root(None)])]\npub struct AppModule;\n";
        let t = ensure_module_import("crate::posts::PostsHttpModule", "PostsHttpModule");
        let out = t(src).expect("inserts");
        assert!(out.contains("PostsHttpModule"));
        assert!(out.contains("use crate::posts::PostsHttpModule;"));
    }
}
