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
/// directory**, never listed, so no template escapes the guards below.
/// `include_str!` cannot glob, so the scan is a test-only `read_dir`; the
/// `generate::cargo` sweep reads the same corpus.
#[cfg(test)]
pub(crate) fn sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the templates directory")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
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
    // Finding nothing reads exactly like finding nothing wrong.
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
    /// project never reads; the placeholder is the only legal form.
    #[test]
    fn templates_use_the_env_prefix_placeholder_not_a_literal() {
        let scanned = sources();
        let literals: Vec<&str> = scanned
            .iter()
            .flat_map(|(_, src)| src.lines())
            // Comments in the CLI's own source describe the scheme; only the emitted
            // template strings are the contract.
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains("NESTRS_"))
            .map(str::trim)
            .collect();
        assert!(
            literals.is_empty(),
            "templates must write {{{{env_prefix}}}}_, not a literal: {literals:#?}",
        );
    }

    /// `container.md`, *Every wait the framework owns is bounded*: a scaffolded
    /// binary's `main` is `#[nest_rs::main]`, never `#[tokio::main]`.
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

    /// `CLAUDE.md`: a log line carries at least one field.
    ///
    /// Matches the macro-call shape (`tracing::<level>!(target: …`), so prose
    /// mentioning `tracing::` never trips it; the next field must come before the
    /// message's opening quote.
    #[test]
    fn no_scaffolded_log_is_emitted_without_a_structured_field() {
        let scanned = sources();
        let bare: Vec<&str> = scanned
            .iter()
            .flat_map(|(_, src)| src.lines())
            .filter(|line| line.contains("tracing::") && line.contains("!(target: "))
            .filter(|line| {
                line.split_once("!(target: ")
                    .and_then(|(_, rest)| rest.split_once(','))
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

    /// `CLAUDE.md` hard "no": a `Debug` over a credential is one `?field` away
    /// from a log line, so no scaffolded struct derives it over one.
    #[test]
    fn no_scaffolded_struct_derives_debug_over_a_credential() {
        const CREDENTIALS: [&str; 4] = ["token", "password", "secret", "credential"];

        let scanned = sources();
        let mut derives = 0;
        let mut exposed = Vec::new();
        for (file, src) in &scanned {
            let mut debug = false;
            let mut fields = false;
            for line in src.lines().map(str::trim) {
                if line.starts_with("#[derive(") {
                    debug |= line.contains("Debug");
                } else if fields {
                    if line == "}" {
                        fields = false;
                    } else if let Some((name, _)) = line.trim_start_matches("pub ").split_once(':')
                        && name.split('_').any(|word| CREDENTIALS.contains(&word))
                    {
                        exposed.push(format!("{file}: `{line}`"));
                    }
                } else if line.starts_with("pub struct ") || line.starts_with("struct ") {
                    derives += usize::from(debug);
                    fields = debug && line.ends_with('{');
                    debug = false;
                } else if !line.starts_with("#[") {
                    debug = false;
                }
            }
        }
        assert!(
            derives >= 2,
            "the scan found {derives} scaffolded structs deriving `Debug` — it stopped matching",
        );
        assert!(
            exposed.is_empty(),
            "a scaffolded `Debug` prints these credentials:\n{}",
            exposed.join("\n"),
        );
    }

    /// `CLAUDE.md`: a target is a constant its owner declares, never a literal at
    /// the call site — a feature's files log on `crate::<feature>::TARGET`.
    #[test]
    fn no_scaffolded_log_spells_its_target_as_a_literal() {
        let scanned = sources();
        let literal: Vec<&str> = scanned
            .iter()
            .flat_map(|(_, src)| src.lines())
            .filter(|line| line.contains("!(target: \""))
            .map(|line| line.trim())
            .collect();

        assert!(
            literal.is_empty(),
            "these scaffolded logs spell their target as a literal:\n{}",
            literal.join("\n"),
        );
    }
}
