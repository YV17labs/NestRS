//! The queue capabilities join: every optional capability a queue backend may declare,
//! against the three places each one has to be answered.
//!
//! An optional capability is a family from its second member, and each member
//! owes the same three cells:
//!
//! - **the refusal**, proved in the port's own suite against a backend declaring
//!   none — a test under `crates/nest-rs-queue/tests/` whose name spells the
//!   member. A backend that cannot honour a declaration owes a sentence, and this
//!   is the cell that keeps the sentence from silently becoming a no-op;
//! - **the behaviour**, proved on every first-party backend that declares the
//!   member — a test under that crate's `tests/e2e/` whose name spells it. What a
//!   backend declares is read off the `QueueBackend` it constructs, so a capability
//!   claimed without an e2e is a hole, and one never claimed owes nothing there;
//! - **the page**, a docs page naming the member the way a backend declares it,
//!   `Capability::<Member>`.
//!
//! Members are derived from `nest_rs_queue::Capability`'s variants, and a member
//! is spelled in a test's name as its variant in snake case —
//! `Capability::DelayedPush` is `delayed_push`.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, files_with_extension, flatten, parsed, read, relative, repo_root, rust_files,
};
use proc_macro2::TokenTree;
use quote::ToTokens;
use syn::{Item, ItemFn};

const BASELINE: &str = "queue-capabilities-baseline.txt";

/// The variants `Capability` declares today. Below this the scan is reading the
/// wrong file.
const FLOOR: usize = 6;

#[test]
fn every_queue_capability_is_refused_by_the_port_proved_on_its_backends_and_documented() {
    let root = repo_root();
    let members = capability_variants(&root.join("crates/nest-rs-queue/src/capability.rs"));
    baseline::floor(members.len(), FLOOR, "queue capabilities");

    let port_tests = test_names(&root.join("crates/nest-rs-queue/tests"));
    let backends = declared_backends(&root);
    let pages: Vec<String> = files_with_extension(&root.join("docs/src/content/docs"), "mdx")
        .iter()
        .filter_map(|page| read(page).ok())
        .collect();

    let mut holes = BTreeSet::new();
    for member in &members {
        let spelled = snake_case(member);
        if !port_tests.iter().any(|name| name.contains(&spelled)) {
            holes.insert(format!("{member} :: refusal in nest-rs-queue's suite"));
        }
        for backend in backends
            .iter()
            .filter(|backend| backend.declares.contains(member))
        {
            if !backend.e2e_tests.iter().any(|name| name.contains(&spelled)) {
                holes.insert(format!("{member} :: e2e in {}", backend.crate_path));
            }
        }
        let declared = format!("Capability::{member}");
        if !pages.iter().any(|page| page.contains(&declared)) {
            holes.insert(format!("{member} :: docs page naming `{declared}`"));
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        members.len(),
        "queue capabilities",
        "capability cells no test or page fills",
        "a capability a backend may declare with no refusal test in the port, no \
         e2e on a backend that claims it, or no page naming it — write the test \
         whose name spells the capability in snake case, or the page",
    );
}

/// A crate constructing a `QueueBackend`, what that construction declares, and
/// the names of its e2e tests.
struct Backend {
    crate_path: String,
    declares: BTreeSet<String>,
    e2e_tests: BTreeSet<String>,
}

/// The variants of the `Capability` enum in `file`.
fn capability_variants(file: &Path) -> Vec<String> {
    let Some(ast) = parsed(file) else {
        return Vec::new();
    };
    ast.items
        .iter()
        .find_map(|item| match item {
            Item::Enum(declared) if declared.ident == "Capability" => Some(
                declared
                    .variants
                    .iter()
                    .map(|variant| variant.ident.to_string())
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// Every crate other than the port whose `src/` holds a `static` or `const`
/// `QueueBackend`, with the capabilities its initializer names.
fn declared_backends(root: &Path) -> Vec<Backend> {
    let mut backends = Vec::new();
    for dir in crate_dirs() {
        if dir.ends_with("nest-rs-queue") {
            continue;
        }
        let mut declares = BTreeSet::new();
        for file in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            for item in &ast.items {
                let (ty, expr) = match item {
                    Item::Static(declared) => (&declared.ty, &declared.expr),
                    Item::Const(declared) => (&declared.ty, &declared.expr),
                    _ => continue,
                };
                if !ty.to_token_stream().to_string().ends_with("QueueBackend") {
                    continue;
                }
                declares.extend(capability_paths(expr.to_token_stream()));
            }
        }
        if declares.is_empty() {
            continue;
        }
        backends.push(Backend {
            crate_path: relative(&dir, root),
            declares,
            e2e_tests: test_names(&dir.join("tests/e2e")),
        });
    }
    backends
}

/// Every `Capability::<Variant>` a token stream spells.
fn capability_paths(tokens: proc_macro2::TokenStream) -> BTreeSet<String> {
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);
    flat.windows(4)
        .filter_map(|window| match window {
            [
                TokenTree::Ident(owner),
                TokenTree::Punct(first),
                TokenTree::Punct(second),
                TokenTree::Ident(variant),
            ] if owner == "Capability" && first.as_char() == ':' && second.as_char() == ':' => {
                Some(variant.to_string())
            }
            _ => None,
        })
        .collect()
}

/// The names of every `#[test]` and `#[tokio::test]` function under `dir`,
/// nested modules included.
fn test_names(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for file in rust_files(dir) {
        if let Some(ast) = parsed(&file) {
            collect_tests(&ast.items, &mut names);
        }
    }
    names
}

fn collect_tests(items: &[Item], names: &mut BTreeSet<String>) {
    for item in items {
        match item {
            Item::Fn(function) if is_test(function) => {
                names.insert(function.sig.ident.to_string());
            }
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_tests(nested, names);
                }
            }
            _ => {}
        }
    }
}

fn is_test(function: &ItemFn) -> bool {
    function.attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "test")
    })
}

/// `DelayedPush` as a test name spells it: `delayed_push`.
fn snake_case(variant: &str) -> String {
    let mut spelled = String::new();
    for (at, character) in variant.char_indices() {
        if character.is_ascii_uppercase() {
            if at > 0 {
                spelled.push('_');
            }
            spelled.push(character.to_ascii_lowercase());
        } else {
            spelled.push(character);
        }
    }
    spelled
}
