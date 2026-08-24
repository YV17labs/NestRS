//! The grammars join: every decorator whose arguments are `key = value` pairs,
//! against the four refusals such a grammar owes.
//!
//! `CLAUDE.md`: *"Where the standard does not [permit it], that key is a
//! **compile error naming the fact that makes it impossible** — never an ignored
//! argument, never a bare 'unknown key'."* And the testable form, one paragraph
//! down: *"**Refusals are shared, not per key.** One helper, one sentence, every
//! key it covers, one trybuild snapshot per site. Per-key refusals multiply with
//! the matrix, and what multiplies is what gets skipped."*
//!
//! **This family had no join, and that is why six silences were live at once.**
//! Every other family here caught its holes the day a join landed. The four
//! sentences live in `nest_rs_codegen::args`, seven decorators adopted all of
//! them, and four adopted none — `#[expose]`, `#[api]`, GraphQL's `#[authorize]`
//! and `#[inject]` each took a repeated key and dropped one of the two
//! declarations by source order. On `#[api]` that is published prose
//! (`CLAUDE.md` mandates the attribute *instead of* a doc comment); on
//! `#[authorize]` it is which service loads the authorized subject.
//!
//! Members are derived, never listed: a decorator is in the population when a
//! `*-macros` crate names it in an `unknown_argument` or `unmatched_meta` call,
//! which is precisely "this decorator has a key set it refuses strangers
//! against". A decorator taking no arguments at all is not in the family and
//! owes none of this — `DecoratorPair::reject_args` is its whole grammar, and
//! the `shapes` join owns that.
//!
//! **Two obligations, not four columns**, and the merge is argued rather than a
//! shortcut: `unmatched_meta` answers the bare-key and unknown-key questions in
//! one call by design ("the two questions are answered here, together, once"),
//! so a site adopting it satisfies both, and a site hand-rolling either owes
//! both separately. The join therefore asks for the *sentence*, whichever helper
//! produced it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{files_with_extension, flatten, read, repo_root, rust_files};
use proc_macro2::{Delimiter, TokenStream, TokenTree};

const BASELINE: &str = "grammars-baseline.txt";

/// Decorators with a `key = value` grammar. Below this the scan is reading the
/// wrong tree.
const FLOOR: usize = 10;

/// Every decorator a `*-macros` crate refuses an unknown key for, mapped to the
/// crates that word it.
///
/// The literal is the decorator's *name without brackets* — `unknown_argument`
/// formats it as `#[{attr}]` — so it is read straight out of the call's first
/// argument.
fn grammars(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for dir in macro_crates(root) {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        for file in rust_files(&dir.join("src")) {
            let Ok(text) = read(&file) else {
                continue;
            };
            for attr in string_args_of(&text, &["unknown_argument", "unmatched_meta"]) {
                out.entry(attr).or_default().insert(krate.clone());
            }
        }
    }
    out
}

/// `crates/*-macros`, plus `nest-rs-codegen` — which holds `#[inject]`'s own
/// grammar and is where the sentences are worded, so excluding it would exempt
/// the one site best placed to know better.
fn macro_crates(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![root.join("crates/nest-rs-codegen")];
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-macros"))
        {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Every string-literal argument of every call to one of `names`.
///
/// **All of them, not the first**, because the position differs by helper:
/// `unknown_argument(attr, …)` leads with it while
/// `reject_duplicate_argument(taken, at, attr, name)`
/// carries it third, behind a `bool` and a token span. Reading position 0 said
/// no decorator refuses a repeat, including the five that had just been taught
/// to — a scan that answers "none" for a whole column is indistinguishable from
/// a column nobody implemented.
///
/// Read as tokens because these calls sit inside `quote!` bodies and behind
/// `nest_rs_codegen::` paths alike, and because a text scan cannot tell a call
/// from the doc comment above it — `args.rs` names four of its own helpers in
/// prose, which a `contains` would enrol as call sites.
fn string_args_of(text: &str, names: &[&str]) -> Vec<String> {
    let Ok(tokens) = text.parse::<TokenStream>() else {
        return Vec::new();
    };
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);
    let mut out = Vec::new();
    for window in flat.windows(2) {
        let [TokenTree::Ident(called), TokenTree::Group(args)] = window else {
            continue;
        };
        if args.delimiter() != Delimiter::Parenthesis
            || !names.contains(&called.to_string().as_str())
        {
            continue;
        }
        for tree in args.stream() {
            if let TokenTree::Literal(lit) = tree
                && let Ok(text) = syn::parse_str::<syn::LitStr>(&lit.to_string())
            {
                out.push(text.value());
            }
        }
    }
    out
}

/// Whether any crate wording this decorator's grammar also refuses a repeat.
///
/// `reject_duplicate_argument` and `duplicate_argument` are the two spellings
/// of one sentence — the
/// guard and the message — and either is the adoption.
fn refuses_a_repeat(root: &Path, attr: &str, crates: &BTreeSet<String>) -> bool {
    macro_crates(root)
        .into_iter()
        .filter(|dir| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| crates.contains(n))
        })
        .flat_map(|dir| rust_files(&dir.join("src")))
        .filter_map(|file| read(&file).ok())
        .any(|text| {
            string_args_of(&text, &["reject_duplicate_argument", "duplicate_argument"])
                .iter()
                .any(|named| named == attr)
        })
}

/// Whether a trybuild snapshot anywhere in the tree pins one of this
/// decorator's grammar refusals.
///
/// The `.stderr`, because that is the compiler actually saying it: a helper
/// called in a `src/` file proves the code exists, and only a snapshot proves it
/// is reachable and worded as recorded. Matched on the two shared sentence
/// shapes rather than on a file name — `testing.md` clause 3, and this join's
/// siblings record a rename closing a cell that asked for a name.
fn snapshotted(root: &Path, needles: &[String]) -> bool {
    files_with_extension(&root.join("crates"), "stderr")
        .into_iter()
        .filter_map(|path| read(&path).ok())
        .any(|text| needles.iter().any(|needle| text.contains(needle)))
}

#[test]
fn every_key_value_grammar_refuses_the_four_ways_of_getting_it_wrong() {
    let root = repo_root();
    let grammars = grammars(&root);
    baseline::floor(
        grammars.len(),
        FLOOR,
        "decorator(s) with a key = value grammar",
    );

    let mut holes = BTreeSet::new();
    let mut cells = 0usize;
    for (attr, crates) in &grammars {
        let duplicate = format!("#[{attr}] takes at most one ");
        let bare = format!("#[{attr}] `");
        let unknown = format!("unknown #[{attr}] argument ");
        for (column, present) in [
            (
                "a duplicate key refused, through the shared `reject_duplicate_argument`",
                refuses_a_repeat(&root, attr, crates),
            ),
            (
                "a snapshot pinning the duplicate-key refusal",
                snapshotted(&root, &[duplicate]),
            ),
            (
                "a snapshot pinning the unknown-key refusal",
                snapshotted(&root, &[unknown]),
            ),
            (
                "a snapshot pinning the bare-key refusal",
                snapshotted(&root, &[bare]),
            ),
        ] {
            cells += 1;
            if !present {
                // **The crate first, and not only for context.** A baseline
                // line starting with `#` is a comment to `baseline::compare`,
                // and a cell keyed `#[authorize] :: …` is exactly that — the
                // two recorded holes read as prose and the join reported them
                // as new on every run. Naming the crate that words the grammar
                // is also the more useful key: it says where to go.
                let owners = crates.iter().cloned().collect::<Vec<_>>().join(", ");
                holes.insert(format!("{owners} #[{attr}] :: {column}"));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        cells,
        "cells",
        "decorator × grammar refusal",
        "a way of writing the arguments wrong that this decorator does not name",
    );
}
