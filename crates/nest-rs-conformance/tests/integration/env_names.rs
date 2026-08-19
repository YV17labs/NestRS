//! The env-name join: every place the framework spells a `<PREFIX>_…` variable,
//! against the rule that a name is **built** and never written.
//!
//! `CLAUDE.md`'s hard "no": *"a name is always built — `nest_rs_config::var_name(ns, key)`
//! or `EnvPrefix::var(name)`, never `"NESTRS_AUTHN__SECRET"` in a message, a
//! check or a template."* `NESTRS` is the *deployment's* default, so
//! `NESTRS_ENV_PREFIX=ACME` renames every framework variable at once — and a
//! literal points at nothing from that moment, with the compiler silent.
//!
//! It was a rule with no join, and the cost was measurable: under
//! `NESTRS_ENV_PREFIX=ACME` **70 tests across 15 crates failed**, and the ones
//! that did not fail passed by asserting a name nobody sets. The production code
//! was clean throughout — every error message named the variable correctly —
//! so the whole of it was suites checking a spelling the framework had already
//! stopped using.
//!
//! Members are derived, never listed: every `NESTRS_`-prefixed string literal
//! under `crates/` and `demo/`. The three sanctioned exceptions are stated
//! below, each because the name is not the application's to rename.

use std::collections::BTreeSet;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{crate_dirs, parsed, relative, repo_root, rust_files};

const BASELINE: &str = "env-names-baseline.txt";

/// Files the population walk must parse. Below this it is reading the wrong
/// tree — and a join that finds nothing reads exactly like a join that found
/// nothing wrong, so this is the only thing standing between the two.
///
/// A file count rather than a hit count, because the healthy state of this join
/// is **zero** hits: nothing else it produces can witness that the walk ran.
/// The walk parses 834 files today — 629 under `src` and 447 under `tests`
/// less the trybuild fixtures, which deliberately do not parse, and the paths
/// [`is_prose`] skips. Dropping either subdirectory takes it below this, which
/// is exactly the mutation that went unnoticed.
const FLOOR: usize = 700;

/// The names no prefix can rename, and why each is not the application's.
///
/// `NESTRS_ENV_PREFIX` is the bootstrap's own — it is what *chooses* the prefix,
/// so it cannot be spelled through one; `CLAUDE.md` sanctions it and requires it
/// be spelled once per crate (`EnvPrefix::VAR`, `context::ENV_PREFIX_VAR`) and
/// referenced from there. `NESTRS_NO_BOOTSTRAP` is the CLI tool's own, not an
/// app's. `RUST_LOG` is the ecosystem's and carries no prefix at all, so it
/// never matches this scan.
const SANCTIONED: [&str; 2] = ["NESTRS_ENV_PREFIX", "NESTRS_NO_BOOTSTRAP"];

/// Where a name is legitimately written as text rather than built.
///
/// A `.env` file, a Dockerfile and a docs page are **data an operator reads**,
/// not code the framework interprets — the deployment that renames its prefix
/// rewrites them, and there is nothing to build a name from. The CLI's templates
/// are the generated project's source and carry their own rule
/// (`templates/shared.rs`), enforced by the CLI's own scan.
fn is_prose(rel: &str) -> bool {
    rel.contains("/templates/")
        || (rel.contains("/tests/") && rel.contains("/diagnostics/"))
        // This join's own source, which necessarily spells the prefix it looks
        // for. Excluding it by path rather than by a cleverer predicate: a scan
        // that cannot see itself is the honest shape, and naming it here is
        // cheaper than a rule that happens to exclude it.
        || rel.ends_with("integration/env_names.rs")
        // `nest-rs-cli` links no `nest-rs-*` crate — that is the whole reason it
        // mirrors `var_name` in `context/workspace.rs` — so its own sources
        // cannot build a name from the framework's authority. The mirror is
        // forced, and the CLI carries its own scan over the templates it emits.
        || rel.starts_with("crates/nest-rs-cli/")
        // The one suite whose *subject* is the prefix. Proving that the
        // un-prefixed name is inert under `NESTRS_ENV_PREFIX=ACME` requires
        // writing it — there is nothing to build it from, because the whole
        // point is that the framework no longer names it. Spelled once, in the
        // test that asserts it reads nothing.
        //
        // Hand-written and argued, in the same shape and for the same reason
        // `guards.rs`'s `attested_marker` is: forgetting to list a file here
        // reports a hole that is not one, which is the safe direction.
        || rel.ends_with("integration/env_prefix.rs")
}

/// A literal spelling a framework variable, as `crate/file :: literal`, and the
/// number of files the walk actually parsed.
///
/// **The count is returned because the floor has to be on this walk**, not on
/// another one. It was on a separate pass that re-read `src/` for the sanctioned
/// names, sharing nothing with the population — not `parsed`, not [`Spelled`],
/// not [`is_prose`], and **not the `tests` subdirectory**. Narrowing `sub` to
/// `["src"]` therefore dropped the entire half where every historical violation
/// lived, took a seeded one with it, and the floor stayed green. `filters.rs`
/// records the same correction in the other direction — "Merging first made this
/// unfireable" — and `events`, `units` and `panics` all floor on the walk under
/// test.
fn spelled_names(root: &std::path::Path) -> (BTreeSet<String>, usize) {
    let mut out = BTreeSet::new();
    let mut parsed_files = 0usize;
    for dir in crate_dirs() {
        for sub in ["src", "tests"] {
            for path in rust_files(&dir.join(sub)) {
                let rel = relative(&path, root);
                if is_prose(&rel) {
                    continue;
                }
                let Some(ast) = parsed(&path) else {
                    continue;
                };
                parsed_files += 1;
                let mut found = Spelled::default();
                syn::visit::Visit::visit_file(&mut found, &ast);
                for literal in found.0 {
                    if !literal.contains("NESTRS_") {
                        continue;
                    }
                    if SANCTIONED.iter().any(|s| literal.starts_with(s)) {
                        continue;
                    }
                    out.insert(format!("{rel} :: {literal}"));
                }
            }
        }
    }
    (out, parsed_files)
}

/// Every string literal an item spells, **attributes excluded**.
///
/// `syn` lowers `///` and `//!` to `#[doc = "…"]`, so a token scan reads prose
/// *about* a variable as a spelling of it. That is not what the rule covers:
/// `CLAUDE.md` names "a message, a check or a template", and the doc-comment
/// clause exists only in `nest-rs-cli/src/templates/shared.rs`, which is the
/// **generated project's** rule and is enforced by the CLI's own scan. The two
/// wordings differ and that difference is reported to the owner rather than
/// resolved here — so this join holds the narrower one, which is the repo's.
///
/// Same reason and same mechanism as `events.rs`'s `Asserted::visit_attribute`.
///
/// **Macro bodies are descended into, and leaving them out was the hole.**
/// `syn` keeps a macro invocation's arguments as an opaque `TokenStream`, so an
/// expression visitor never reaches a literal inside `assert!(…)` or
/// `format!(…)` — which is exactly where a *test* spells a variable name. This
/// join read clean over four crates that spelled one, and the run under
/// `NESTRS_ENV_PREFIX=ACME` failed on them: the join was reporting the absence
/// of the literals it could not see. Same correction `events.rs` already made
/// for `macro_rules!`, in the opposite direction.
#[derive(Default)]
struct Spelled(Vec<String>);

impl Spelled {
    /// Every string literal in a token stream, at any group depth.
    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for tree in tokens {
            match tree {
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                proc_macro2::TokenTree::Literal(literal) => {
                    if let Ok(lit) = syn::parse_str::<syn::LitStr>(&literal.to_string()) {
                        self.0.push(lit.value());
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for Spelled {
    fn visit_attribute(&mut self, _node: &'ast syn::Attribute) {}

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.tokens(node.tokens.clone());
    }

    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        self.0.push(node.value());
    }
}

#[test]
fn no_framework_variable_is_spelled_as_a_literal() {
    let root = repo_root();
    let (spelled, parsed_files) = spelled_names(&root);
    baseline::floor(
        parsed_files,
        FLOOR,
        "file(s) parsed by this join's own walk",
    );

    baseline::gate(
        BASELINE,
        &spelled,
        spelled.len().max(1),
        "spellings",
        "framework variable spelled as a literal",
        "a name written where it must be built",
    );
}
