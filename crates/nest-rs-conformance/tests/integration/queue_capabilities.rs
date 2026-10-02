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
//!   backend declares is every `Capability::<Member>` its crate spells, once the
//!   crate constructs a `QueueBackend` anywhere — so a capability claimed without
//!   an e2e is a hole, and one never named owes nothing there;
//! - **the page**, a docs page naming the member the way a backend declares it,
//!   `Capability::<Member>`.
//!
//! Members are derived from `nest_rs_queue::Capability`'s variants, and a member
//! is spelled in a test's name as its variant in snake case —
//! `Capability::DelayedPush` is `delayed_push`.
//!
//! **What it reads by its spelling**: `QueueBackend::new` / `QueueBackend { … }`
//! and `Capability::<Member>`, anywhere in a crate's tokens — a `macro_rules!`
//! transcriber's included. What would hide one is refused by the `blinds` join:
//! a rename or a `type` alias of either, an import through `Capability`
//! (`use …::Capability::*`, `use …::Capability::Throttle`), and a member a
//! transcriber's caller chooses (`Capability::$v`).

use std::collections::BTreeSet;
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, files_with_extension, flatten, is_cfg_test, parsed, read, relative, repo_root,
    rust_files,
};
use proc_macro2::TokenTree;
use quote::ToTokens;
use syn::{Item, ItemFn};

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![
        Followed::type_(CAPABILITY).read_in_transcribers(),
        Followed::type_(BACKEND).read_in_transcribers(),
    ]
}

const BASELINE: &str = "queue-capabilities-baseline.txt";

/// The enum a capability is a variant of, read as the head of `Capability::X`.
const CAPABILITY: &str = "Capability";

/// The type a backend is constructed as, read as `QueueBackend::new` or a
/// `QueueBackend { … }` literal.
const BACKEND: &str = "QueueBackend";

/// The variants `Capability` declares today. Below this the scan is reading the
/// wrong file.
const FLOOR: usize = 5;

#[test]
fn every_queue_capability_is_refused_by_the_port_proved_on_its_backends_and_documented() {
    let root = repo_root();
    let members = capability_variants(&root.join("crates/nest-rs-queue/src/capability.rs"));
    baseline::floor(members.len(), FLOOR, "queue capabilities");

    let port_tests = test_names(&root.join("crates/nest-rs-queue/tests"));
    let backends = declared_backends(&root, &crate_dirs());
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
        let declared = format!("{CAPABILITY}::{member}");
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
            Item::Enum(declared) if declared.ident == CAPABILITY => Some(
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

/// Every crate other than the port that constructs a `QueueBackend`, with every
/// capability its source names.
///
/// **The crate, not the initializer.** Reading only a top-level `static` or
/// `const` typed `QueueBackend` and the `Capability::X` tokens inside it missed a
/// set named through a constant (`QueueBackend::new("zz", CAPS)`), and a backend
/// built in a function, an impl's `const` or an inline module: the claim was
/// real and the crate owed no e2e. A crate constructing one anywhere now owes
/// the behaviour for every capability it spells anywhere outside its tests —
/// the direction that over-asks rather than lets a claim through.
fn declared_backends(root: &Path, dirs: &[std::path::PathBuf]) -> Vec<Backend> {
    let mut backends = Vec::new();
    for dir in dirs {
        if dir.ends_with("nest-rs-queue") {
            continue;
        }
        let mut constructs = false;
        let mut declares = BTreeSet::new();
        for file in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            let tokens: proc_macro2::TokenStream = ast
                .items
                .iter()
                .filter(|item| !item_is_cfg_test(item))
                .map(ToTokens::to_token_stream)
                .collect();
            constructs |= constructs_a_backend(&tokens);
            declares.extend(capability_paths(tokens));
        }
        if !constructs || declares.is_empty() {
            continue;
        }
        backends.push(Backend {
            crate_path: relative(dir, root),
            declares,
            e2e_tests: test_names(&dir.join("tests/e2e")),
        });
    }
    backends
}

/// Whether an item sits behind `#[cfg(test)]` — the only items a crate compiles
/// for its unit tests alone.
fn item_is_cfg_test(item: &Item) -> bool {
    match item {
        Item::Mod(i) => is_cfg_test(&i.attrs),
        Item::Fn(i) => is_cfg_test(&i.attrs),
        Item::Const(i) => is_cfg_test(&i.attrs),
        Item::Static(i) => is_cfg_test(&i.attrs),
        Item::Impl(i) => is_cfg_test(&i.attrs),
        _ => false,
    }
}

/// `QueueBackend::new(…)` or a `QueueBackend { … }` literal, anywhere.
fn constructs_a_backend(tokens: &proc_macro2::TokenStream) -> bool {
    let mut flat = Vec::new();
    flatten(tokens.clone(), &mut flat);
    flat.windows(4).any(|window| match window {
        [
            TokenTree::Ident(ty),
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            TokenTree::Ident(ctor),
        ] => ty == BACKEND && first.as_char() == ':' && second.as_char() == ':' && ctor == "new",
        [TokenTree::Ident(ty), TokenTree::Group(body), ..] => {
            ty == BACKEND && body.delimiter() == proc_macro2::Delimiter::Brace
        }
        _ => false,
    })
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
            ] if owner == CAPABILITY && first.as_char() == ':' && second.as_char() == ':' => {
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

/// The backend read on a planted tree: a capability set named through a
/// constant and a backend built in a function are both claims, and a crate that
/// constructs no backend claims nothing whatever it names.
#[test]
fn a_backend_claims_what_its_crate_names_however_it_builds_the_set() {
    const TREE: [(&str, &str); 3] = [
        (
            "crates/nest-rs-alpha/src/backend.rs",
            "const CAPS: Capabilities = Capabilities::NONE.with(Capability::Throttle);\n\
             static BACKEND: QueueBackend = QueueBackend::new(\"alpha\", CAPS);\n\
             #[cfg(test)] mod tests { const X: Capability = Capability::Checkpoint; }\n",
        ),
        (
            "crates/nest-rs-beta/src/lib.rs",
            "pub fn backend() -> QueueBackend {\n\
                 QueueBackend::new(\"beta\", Capabilities::NONE.with(Capability::DelayedPush))\n\
             }\n",
        ),
        (
            "crates/nest-rs-gamma/src/lib.rs",
            "pub fn refuses(c: Capability) -> bool { c == Capability::UniquePush }\n",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("queue-capabilities-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let dirs = ["alpha", "beta", "gamma"].map(|name| root.join(format!("crates/nest-rs-{name}")));
    let read: Vec<(String, Vec<String>)> = declared_backends(&root, &dirs)
        .into_iter()
        .map(|backend| (backend.crate_path, backend.declares.into_iter().collect()))
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        read,
        [
            (
                "crates/nest-rs-alpha".to_owned(),
                vec!["Throttle".to_owned()]
            ),
            (
                "crates/nest-rs-beta".to_owned(),
                vec!["DelayedPush".to_owned()]
            ),
        ],
    );
}
