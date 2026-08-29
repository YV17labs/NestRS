use std::path::Path;

use syn::Item;

use super::Finding;
use crate::naming::{reserved_category, to_kebab};

/// Stems that are not vocabulary, so are not this rule's business.
///
/// The role words (`module`, `service`, `guard`, …) and the pluralized role
/// folders (`entities/`, `dtos/`, …) are **not** listed here: they are derived
/// from the *Reserved vocabulary* block of `architecture.md` through
/// [`reserved_category`] — the same derivation `nestrs new` refuses a feature
/// name with — so a row added to that table reaches this rule and that refusal
/// at once, and neither can drift from the file both are written in.
///
/// What is left is what the block does not carry: Rust's own file names, and
/// the recognised custom-provider words, which *architecture.md* names in the
/// prose of its custom-provider paragraph rather than in the table. Those take
/// that paragraph's own pairing (`<Subject>Registry`), so they are not this
/// rule's business either.
const NOT_VOCABULARY: &[&str] = &[
    "lib",
    "main",
    "build",
    "exception_filter",
    "registry",
    "client",
    "store",
    "factory",
    "source",
    "bridge",
    "inventory",
];

/// Directories a scan never descends into. `migrations` is not an exemption
/// won on merit: sea-orm fixes those filenames to `m<date>_<name>`, so the stem
/// is a timestamp and no pairing was ever available to check.
const SKIPPED: &[&str] = &["target", ".git", "node_modules", "migrations"];

/// What one scan found, and how much it looked at.
///
/// `checked` is reported rather than inferred so a caller can tell an empty
/// finding list apart from a walk that read nothing — a scan pointed at the
/// wrong directory is silently clean, and silence is the one result a
/// conformance suite must never accept.
#[derive(Debug, Default)]
pub struct Scan {
    /// Files that carried a declared type and were subject to the pairing.
    pub checked: usize,
    /// Every file whose stem reached nothing it declares.
    pub findings: Vec<Finding>,
}

/// Walk every `src/` file under `root` and pair each stem against what its file
/// declares.
pub fn scan(root: &Path) -> Scan {
    let mut out = Scan::default();
    walk(root, root, false, &mut out);
    out.findings.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// `in_src` is carried down rather than re-derived per file: only source is
/// judged, and a `tests/` tree names its files for the `src/` concern they
/// cover, which is a different rule with a different table. Reading it off the
/// path instead would also answer *yes* for every tree the caller happens to
/// reach through a directory of their own called `src`.
fn walk(dir: &Path, root: &Path, in_src: bool, out: &mut Scan) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `file_type` reads what the directory listing already carried, where
        // `is_dir` would pay a `stat` per entry. It does not follow symlinks —
        // deliberate: a symlinked tree is judged where it really lives, once.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !SKIPPED.contains(&name) && !name.starts_with('.') {
                walk(&path, root, in_src || name == "src", out);
            }
        } else if in_src && name.ends_with(".rs") {
            inspect(&path, root, out);
        }
    }
}

fn inspect(path: &Path, root: &Path, out: &mut Scan) {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    if NOT_VOCABULARY.contains(&stem) || reserved_category(stem) == Some("roles") {
        return;
    }
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if reserved_category(folder) == Some("plurals") {
        return;
    }

    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    // Rust's own parser, not a scan: a `pub struct` inside a template string is
    // the exact thing a regex counts and a parser does not, and this crate
    // ships such a string.
    let Ok(file) = syn::parse_file(&source) else {
        return;
    };

    let declared: Vec<String> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(s) => Some(s.ident.to_string()),
            Item::Enum(e) => Some(e.ident.to_string()),
            Item::Trait(t) => Some(t.ident.to_string()),
            _ => None,
        })
        .collect();
    if declared.is_empty() {
        return;
    }
    out.checked += 1;

    let mut subject: Vec<&str> = stem.split('_').collect();
    if folder != "src" {
        subject.extend(folder.split('_'));
    }
    let paired = declared.iter().any(|ty| {
        words_of(ty)
            .iter()
            .any(|word| subject.iter().any(|stem_word| reach(stem_word, word)))
    });
    if paired {
        return;
    }

    // The stem names what a caller *calls*, and the types beside it are that
    // procedure's vocabulary rather than the file's subject.
    if file
        .items
        .iter()
        .any(|item| matches!(item, Item::Fn(f) if !matches!(f.vis, syn::Visibility::Inherited)))
    {
        return;
    }

    out.findings.push(Finding {
        path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
        declared,
    });
}

/// `RedisQueueProducer` → `["redis", "queue", "producer"]`, through the same
/// case splitter `nestrs new` derives a crate name with. Two PascalCase
/// splitters in one crate is how the two come to disagree about an acronym.
fn words_of(ty: &str) -> Vec<String> {
    to_kebab(ty).split('-').map(str::to_owned).collect()
}

/// Whether two words reach each other. Deliberately loose — the rule refuses
/// only the file that reaches *nothing*, so every near miss is a pass:
///
/// - a compound contains the other word (`part` in `multipart`);
/// - an inflection shares a root (`scope` / `scoped`, `log` / `logging`);
/// - an abbreviation is a subsequence from the same letter (`ctx` / `context`).
///
/// A tighter test — the stem as the type's first or last word — reads well and
/// is false on a third of the framework, which is why it is not this one.
fn reach(a: &str, b: &str) -> bool {
    a == b
        || (a.len() >= 2 && b.contains(a))
        || (b.len() >= 2 && a.contains(b))
        || a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count() >= 4
        || abbreviates(a, b)
        || abbreviates(b, a)
}

fn abbreviates(short: &str, long: &str) -> bool {
    if short.len() < 2 || long.len() < 4 || short.len() >= long.len() {
        return false;
    }
    if short.as_bytes()[0] != long.as_bytes()[0] {
        return false;
    }
    let mut rest = long.chars();
    short.chars().all(|c| rest.any(|d| d == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_reaches_its_own_inflections_compounds_and_abbreviation() {
        assert!(reach("scope", "scoped"));
        assert!(reach("logging", "log"));
        assert!(reach("multipart", "part"));
        assert!(reach("context", "ctx"));
        assert!(reach("id", "ids"));
    }

    #[test]
    fn a_word_reaches_nothing_it_shares_no_root_with() {
        assert!(!reach("principal", "desk"));
        assert!(!reach("principal", "operator"));
        assert!(!reach("rate", "throttle"));
    }

    #[test]
    fn a_type_splits_into_the_words_a_stem_is_matched_against() {
        assert_eq!(
            words_of("RedisQueueProducer"),
            ["redis", "queue", "producer"]
        );
        assert_eq!(words_of("Repo"), ["repo"]);
    }

    fn scan_one(dir: &Path, name: &str, body: &str) -> Scan {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(name), body).unwrap();
        scan(dir)
    }

    #[test]
    fn a_stem_that_reaches_nothing_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_one(dir.path(), "principal.rs", "pub struct DeskOperator;");
        assert_eq!(scan.checked, 1);
        assert_eq!(scan.findings.len(), 1);
        assert_eq!(scan.findings[0].path, Path::new("src/principal.rs"));
    }

    #[test]
    fn a_stem_the_type_reaches_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_one(dir.path(), "connection.rs", "pub struct RedisConnection;");
        assert!(scan.findings.is_empty());
    }

    #[test]
    fn a_file_whose_principal_export_is_a_function_owes_no_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_one(
            dir.path(),
            "consume.rs",
            "pub enum Attempt { Done }\npub fn attempt() {}",
        );
        assert!(scan.findings.is_empty());
    }

    #[test]
    fn a_file_a_table_already_names_is_not_this_rules_business() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_one(dir.path(), "registry.rs", "pub struct GuardSpec;");
        assert_eq!(scan.checked, 0);
        assert!(scan.findings.is_empty());
    }

    #[test]
    fn a_declaration_inside_a_string_is_not_a_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_one(
            dir.path(),
            "resource.rs",
            "pub const TEMPLATE: &str = r#\"pub struct Model;\"#;",
        );
        assert_eq!(scan.checked, 0);
        assert!(scan.findings.is_empty());
    }
}
