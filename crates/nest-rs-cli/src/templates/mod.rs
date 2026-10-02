//! Embedded project templates — one module per generated artifact.
//!
//! Every module here is `const` source strings; `crud.rs` holds the one
//! computed set of placeholders, because it varies by transport.

pub(crate) mod adapter;
pub(crate) mod auth;
pub(crate) mod crud;
pub(crate) mod entity;
pub(crate) mod feature;
pub(crate) mod hello;
pub(crate) mod migration;
pub(crate) mod resource;
pub(crate) mod shared;
pub(crate) mod workspace;

/// Every template module, as `(file name, source)` — **read from the
/// directory**, never listed. A list is edited by a different hand than the one
/// that adds a template, and the list this replaced had already lost `crud.rs`:
/// the module that renders every adapter's handler body sat outside both guards
/// below while the doc above them claimed the population was scanned.
///
/// `include_str!` cannot glob, so the scan is a `read_dir` at test time. It is
/// `#[cfg(test)]`-only, so nothing ships a runtime directory read.
///
/// **One scan, three guards.** `generate::cargo`'s Rust-floor sweep reads the
/// same corpus, and the two spelled it apart: this one excluded `mod.rs` and
/// checked a floor, that one did neither — so a change to what counts as a
/// template file had to be made twice, or one guard silently stopped seeing
/// part of the corpus. Which is the failure this scan exists to prevent.
#[cfg(test)]
pub(crate) fn sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the templates directory")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
        // The folder index is not a template: it declares the modules the
        // others are.
        .filter(|path| path.file_name().and_then(|n| n.to_str()) != Some("mod.rs"))
        .collect();
    files.sort();
    let found: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            (
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned(),
                std::fs::read_to_string(path).expect("a template source"),
            )
        })
        .collect();
    // Finding nothing reads exactly like finding nothing wrong, so the scan
    // says it is still matching.
    assert!(
        found.len() >= 10,
        "the templates scan found {} modules — it stopped matching, and a \
         template added since would now pass every guard unread",
        found.len(),
    );
    found
}

#[cfg(test)]
mod tests {
    use super::sources;

    /// A template that spells `NESTRS_` writes a variable an `--env-prefix`
    /// project never reads — a `.env` key silently inert, or a generated tool
    /// looking at the wrong name. The placeholder is the only legal form, so
    /// the guard is mechanical rather than a review habit.
    #[test]
    fn templates_use_the_env_prefix_placeholder_not_a_literal() {
        let scanned = sources();
        let literals: Vec<&str> = scanned
            .iter()
            .flat_map(|(_, src)| src.lines())
            // Rust doc/line comments in the CLI's own source describe the
            // scheme; only the emitted template strings are the contract.
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains("NESTRS_"))
            .map(str::trim)
            .collect();
        assert!(
            literals.is_empty(),
            "templates must write {{{{env_prefix}}}}_, not a literal: {literals:#?}",
        );
    }

    /// `CLAUDE.md`, *No wait the framework does not bound*: a scaffolded
    /// binary's `main` is `#[nest_rs::main]`, which tears the runtime down within
    /// the shutdown budget, never `#[tokio::main]`, whose runtime drop waits on
    /// whatever an abandoned unit left blocking. The repository's own sources
    /// are held to it by the conformance suite's `entries` join; templates are
    /// strings, so they are held here.
    #[test]
    fn every_scaffolded_entry_point_runs_on_nest_rs_main() {
        /// The app's `main`, the migration runner's and the seed's.
        const FLOOR: usize = 3;

        let scanned = sources();
        let mut entries = 0;
        let mut wrong = Vec::new();
        for (file, src) in &scanned {
            let lines: Vec<&str> = src.lines().map(str::trim).collect();
            for (at, line) in lines.iter().enumerate() {
                if line.starts_with("#[tokio::main]") {
                    wrong.push(format!("{file}:{} is `#[tokio::main]`", at + 1));
                }
                if line.starts_with("async fn main(") {
                    entries += 1;
                    let above = at.checked_sub(1).and_then(|above| lines.get(above));
                    if above != Some(&"#[nest_rs::main]") {
                        wrong.push(format!(
                            "{file}:{} is an `async fn main` without `#[nest_rs::main]` above it",
                            at + 1,
                        ));
                    }
                }
            }
        }
        assert!(
            entries >= FLOOR,
            "the scan found {entries} scaffolded `async fn main` — it stopped matching",
        );
        assert!(
            wrong.is_empty(),
            "a scaffolded entry point whose runtime's teardown nothing bounds:\n{}",
            wrong.join("\n"),
        );
    }

    /// `CLAUDE.md`: *metadata is mandatory — a bare log is a defect*. A scaffold
    /// emits what the rules mandate, so a template that logs without a field
    /// ships that defect into every generated project. The whole framework holds
    /// this at zero; the generated code has to as well.
    ///
    /// Matches the macro-call shape (`tracing::<level>!(target: "…"`), so prose
    /// mentioning `tracing::` never trips it.
    #[test]
    fn no_scaffolded_log_is_emitted_without_a_structured_field() {
        let scanned = sources();
        let bare: Vec<&str> = scanned
            .iter()
            .flat_map(|(_, src)| src.lines())
            .filter(|line| line.contains("tracing::") && line.contains("!(target: \""))
            .filter(|line| {
                // Between the target's closing quote and the message's opening
                // one there must be at least one `field = value` pair.
                line.split_once("!(target: \"")
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .is_some_and(|(_, after)| !after.split('"').next().unwrap_or("").contains('='))
            })
            .map(|line| line.trim())
            .collect();

        assert!(
            bare.is_empty(),
            "these scaffolded logs carry no structured field:\n{}",
            bare.join("\n"),
        );
    }
}
