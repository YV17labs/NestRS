//! The umbrella join: every capability the front door offers, against the
//! witnesses `CLAUDE.md` makes it owe.
//!
//! *The umbrella is the front door* fixes what shipping a capability costs — a
//! feature in the matrix, a `pub use`, `cargo add nest-rs --features <x>` in the
//! README **and** the docs page's `## Install`, and two witnesses. It reads as a
//! checklist and was checked by nobody, so a capability could reach a user with
//! no documented way to install it.
//!
//! A **capability** is derived, not listed: a feature whose value activates a
//! `dep:nest-rs-<y>`. That is what separates the twenty-eight capabilities from
//! `default` and `full`, which activate nothing of their own.
//!
//! Six columns, and each is owed by a derived subset rather than by all
//! twenty-eight:
//!
//! - **the re-export**, owed by every capability — `pub use nest_rs_<y> as <y>`,
//!   which is what makes `::nest_rs::<y>::` resolve inside a macro expansion;
//! - **the docs install line**, owed by every capability — some page's
//!   `## Install` spelling `cargo add nest-rs --features <x>`;
//! - **the README**, owed by every capability, and it is *two* checks because
//!   the rule is two sentences: the capability's own README has to spell
//!   `cargo add nest-rs --features <x>`, and no README may tell a reader to
//!   `cargo add nest-rs-<x>` instead. Only the second was implemented once, and
//!   a negative check alone passes vacuously — an empty README corpus reported
//!   zero holes, which is the shape a scan reading the wrong directory takes;
//! - **the expansion witness**, keyed per **decorator** rather than per
//!   capability — every `#[proc_macro_attribute]` any `crates/*-macros` crate
//!   exports owes a use site in `nest-rs-macro-hygiene`, which is 27 cells over
//!   a population of 11 macro crates. Two of those crates
//!   (`nest-rs-core-macros`, `nest-rs-config-macros`) are not capabilities at
//!   all, which is the point: the obligation is the decorator's, and asking it
//!   of the ten capabilities that ship one would have let a crate add a
//!   twenty-eighth decorator with nothing to fill;
//! - **the composition witness**, a boot executed in the owning crate — see
//!   below for why it is this join's column and not the seams join's.
//!
//! **Requirement 4 is stated, not joined, and here is why.** `CLAUDE.md`'s
//! *Shipping a new capability* has five numbered items; this join carries
//! columns for 1, 2, 3 and 5. The fourth — "any derive the decorator emits
//! routed through the surface crate **with its `crate = ` override**, so the
//! use site declares neither the crate nor a version to keep aligned" — is
//! covered by two witnesses that live where the derives do, and a column here
//! would be a third reader of the same fact:
//!
//! - the **rooting** half is executed by
//!   `nest-rs-macro-hygiene/tests/integration/emissions.rs`, which reads every
//!   `*-macros` source and fails on a path rooted outside the framework —
//!   exhaustive over decorators, which a compile witness cannot be;
//! - the **override** half is per decorator, because a `crate = ` attribute's
//!   spelling is the derive's own (`#[serde(crate = "…")]`,
//!   `#[validate(crate = …)]`, `#[schemars(crate = "…")]` are three grammars),
//!   so it is pinned beside each emission — `nest-rs-core-macros`'s
//!   `the_public_rustdoc_names_everything_the_expansion_appends` is the model,
//!   and it exists because the published page showed the call-site form for a
//!   release while the expansion had long been rooted.
//!
//! Stated rather than left out: a join that recites a five-item rule and covers
//! four is exactly the silence *The ask names a site; the design answers the
//! family* forbids — "a member left unstated is the silence these rules exist
//! to forbid".
//!
//! **The composition column is this join's, not the seams join's.** It used to
//! delegate: "the fifth obligation is the seams join's whole subject, and a cell
//! it already reports is closed". That join derives its population from
//! `pub fn for_root` declarations — fourteen of them — while this one derives
//! twenty-eight capabilities, so **sixteen capabilities owned no cell at all**.
//! `CLAUDE.md` states the obligation under *Shipping a new capability*, whose
//! subject is a feature in the matrix; "every `for_root` seam has one" is the
//! *bar*, not the population. A capability with no `for_root` still ships
//! documented wiring, and a boot is still the only thing that proves the access
//! graph, the resolved config and the mounted routes at once. The seams join
//! keeps the sharper question — is the seam *itself* exercised — and this one
//! asks whether the capability boots at all.
//!
//! **One limit of the expansion column, reported rather than papered over**:
//! `nest-rs-macro-hygiene` turns its ten features on in a single build, so a
//! satellite that one capability fails to pull is invisible whenever a
//! neighbouring capability pulls it. This join proves a use site exists; it
//! cannot prove that use site would still compile alone. Splitting the witness
//! per feature is an owner decision, not something to infer from here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, executed_tokens, exported_decorators, files_with_extension, parsed, path_roots,
    read, repo_root, rust_files, spells_path,
};
use syn::{Item, UseTree};

const BASELINE: &str = "umbrella-baseline.txt";

/// Twenty-eight capabilities stand today. Below that the scan is reading the
/// wrong manifest and every hole it reports is an artefact.
const FLOOR: usize = 28;

/// A capability, as the umbrella's own feature matrix declares it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Capability {
    /// The feature name — `server-timing`, the way `--features` spells it.
    feature: String,
    /// The crate it activates — `nest-rs-server-timing`.
    krate: String,
}

impl Capability {
    /// The module path both halves of the re-export use — `server_timing`.
    fn concern(&self) -> String {
        self.krate
            .strip_prefix("nest-rs-")
            .unwrap_or(&self.krate)
            .replace('-', "_")
    }

    /// The cell this capability owes, keyed on the **crate** rather than the
    /// feature.
    ///
    /// One feature may activate two crates — `seaorm` activates
    /// `nest-rs-seaorm` and `nest-rs-resource`, because `#[expose]` expands to
    /// the one and `#[crud]` to the other, so a feature per crate would have to
    /// imply the other and Cargo rejects that as a cycle. Keyed on the feature,
    /// both crates wrote the same cell: holes union, so nothing is hidden while
    /// both are open — but the day one crate gains a boot test the cell closes
    /// for the other too, silently. The crate is what owes a witness, so the
    /// crate is what the cell names.
    fn cell(&self, column: &str) -> String {
        format!("{} :: {column}", self.krate)
    }
}

/// Every umbrella feature that is not a capability and not an aggregate.
///
/// **A feature that ships no crate of its own can still ship a surface**, and
/// that is the class this column exists for. `redis-throttler` forwards
/// `nest-rs-redis/throttler`: it activates no `dep:`, so [`capabilities`] does
/// not see it — yet `nest_rs::redis::RedisThrottlerModule` exists in no build
/// without it, and the manifest's own comment calls it a capability. Owing it
/// the whole checklist would be wrong (it needs no second `pub use`; `redis`
/// already re-exports the module, and no second crate is pulled), but owing it
/// **nothing** left a real surface a reader cannot discover.
///
/// So it owes exactly one thing: an `## Install` line naming it, because that
/// is the difference between a feature a developer can find and one they cannot.
///
/// `default` and `full` are excluded: they activate nothing of their own and
/// are documented as aggregates, which is the same reason [`capabilities`]
/// skips them.
fn sub_features(root: &Path, capabilities: &[Capability]) -> BTreeSet<String> {
    let manifest =
        read(&root.join("crates/nest-rs/Cargo.toml")).expect("the umbrella has a manifest");
    let doc: toml_edit::DocumentMut = manifest
        .parse()
        .expect("the umbrella manifest is valid TOML");
    let owned: BTreeSet<&str> = capabilities.iter().map(|c| c.feature.as_str()).collect();
    let Some(features) = doc.get("features").and_then(|f| f.as_table()) else {
        return BTreeSet::new();
    };
    features
        .iter()
        .map(|(name, _)| name.to_owned())
        .filter(|name| !owned.contains(name.as_str()))
        .filter(|name| name != "default" && name != "full")
        .collect()
}

/// Every capability the umbrella declares, from its `[features]` table.
///
/// Parsed with a TOML parser rather than scanned: a feature list wraps across
/// lines as freely as a Rust string does, and the wrapping is exactly what a
/// line-oriented read gets wrong.
fn capabilities(root: &Path) -> Vec<Capability> {
    let manifest =
        read(&root.join("crates/nest-rs/Cargo.toml")).expect("the umbrella has a manifest");
    let doc: toml_edit::DocumentMut = manifest
        .parse()
        .expect("the umbrella manifest is valid TOML");
    let Some(features) = doc.get("features").and_then(|f| f.as_table()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (feature, value) in features {
        let Some(list) = value.as_array() else {
            continue;
        };
        for entry in list {
            let Some(text) = entry.as_str() else {
                continue;
            };
            // `dep:nest-rs-x` activates the crate outright.
            if let Some(krate) = text.strip_prefix("dep:") {
                out.push(Capability {
                    feature: feature.to_owned(),
                    krate: krate.to_owned(),
                });
                continue;
            }
            // `nest-rs-x/y` and `nest-rs-x?/y` both only forward a feature to a
            // crate some *other* capability activates, so neither makes this
            // feature the owner of a crate. What a strong forward **can** make
            // is a surface reachable only under this feature — see
            // [`sub_features`], which is the column that asks about those.
        }
    }
    out
}

/// The concerns the umbrella re-exports at its root, as `<crate> as <alias>`.
fn reexports(root: &Path) -> BTreeSet<(String, String, Option<String>)> {
    let Some(ast) = parsed(&root.join("crates/nest-rs/src/lib.rs")) else {
        return BTreeSet::new();
    };
    ast.items
        .iter()
        .filter_map(|item| {
            let Item::Use(use_item) = item else {
                return None;
            };
            // `pub use nest_rs_http as http;` — a bare rename, no `::` in it,
            // which is a `UseTree::Rename` at the top of the tree rather than
            // the `Path` a qualified import would give.
            let UseTree::Rename(rename) = &use_item.tree else {
                return None;
            };
            // **`pub`, and gated on the *right* feature.** The column's sentence
            // is that the re-export "is what makes `::nest_rs::<y>::` resolve
            // inside a macro expansion", and neither half of that is true
            // without these two. A dropped `pub` at least trips
            // `unused_imports`; a re-export gated on the **wrong** feature
            // produces no diagnostic in any build — it simply is not there when
            // the expansion needs it, which is `E0433` inside a macro, blamed on
            // the attribute. That is the failure *Shipping a new capability*
            // step 1 exists to prevent, and the cell was closed regardless.
            if !matches!(use_item.vis, syn::Visibility::Public(_)) {
                return None;
            }
            let gate = use_item.attrs.iter().find_map(|attr| {
                let text = quote::ToTokens::to_token_stream(attr).to_string();
                let at = text.find("feature")?;
                text[at..]
                    .split('"')
                    .nth(1)
                    .map(std::string::ToString::to_string)
            });
            Some((rename.ident.to_string(), rename.rename.to_string(), gate))
        })
        .collect()
}

/// Every `cargo add nest-rs --features …` line the docs spell under `## Install`.
///
/// Scoped to that section rather than to the page, because the rule names the
/// section: a feature mentioned in prose lower down is not an install line.
fn documented_installs(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for page in files_with_extension(&root.join("docs/src/content/docs"), "mdx") {
        let Ok(text) = read(&page) else {
            continue;
        };
        let mut inside = false;
        for line in text.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                inside = heading.trim() == "Install";
                continue;
            }
            if !inside {
                continue;
            }
            // Two things a stricter match got wrong. The comma:
            // `--features graphql,seaorm,authz` is one line declaring three
            // capabilities, and that is how the docs write it. And the flags:
            // `testing/index.mdx` says `cargo add --dev nest-rs --features
            // testing`, which is the *correct* advice for a test-only
            // dependency — anchoring on the literal `cargo add nest-rs` read it
            // as a page with no install line at all.
            if !line.contains("cargo add") {
                continue;
            }
            if let Some((_, rest)) = line.split_once("nest-rs --features") {
                out.extend(
                    rest.split([' ', ',', '\t'])
                        .filter(|token| !token.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    out
}

/// Every README in the repo, keyed by the crate that owns it — the crates.io
/// landing pages plus the root's, since the negative half of the rule binds the
/// sentence wherever it appears while the positive half binds one page.
///
/// The root is keyed by the empty string: it belongs to no capability, so it can
/// never satisfy the positive half by accident.
fn readmes(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = read(&root.join("README.md")) {
        out.push((String::new(), text));
    }
    for dir in crate_dirs() {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Ok(text) = read(&dir.join("README.md")) {
            out.push((krate.to_owned(), text));
        }
    }
    assert!(
        out.len() > FLOOR,
        "{} README(s) read — the corpus is the population's own size at least, \
         and a negative column over an empty corpus reports nothing",
        out.len(),
    );
    out
}

/// Every decorator `nest-rs-macro-hygiene` actually **applies**.
///
/// Attributes, not identifiers, and that is the whole correction: the scan read
/// every ident in the crate's sources, so the `mod resolver;` line in its
/// `lib.rs` filled the `#[resolver]` cell. Gutting `src/resolver.rs` of the
/// attribute left the file, the module and this cell exactly as they were —
/// which is the shape `edges.rs` records having fixed in itself
/// (`"authorize.rs".is_file()` closed a cell for a day), and `testing.md`
/// clause 3 asks for a cell that fails when the behaviour goes.
fn hygiene_attrs(root: &Path) -> BTreeSet<String> {
    #[derive(Default)]
    struct Applied(BTreeSet<String>);

    impl<'ast> syn::visit::Visit<'ast> for Applied {
        fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
            if let Some(last) = node.path().segments.last() {
                self.0.insert(last.ident.to_string());
            }
            syn::visit::visit_attribute(self, node);
        }
    }

    let mut applied = Applied::default();
    for path in rust_files(&root.join("crates/nest-rs-macro-hygiene/src")) {
        if let Some(ast) = parsed(&path) {
            syn::visit::Visit::visit_file(&mut applied, &ast);
        }
    }
    applied.0
}

/// Whether a token stream actually boots an app.
fn is_boot(tokens: &proc_macro2::TokenStream) -> bool {
    spells_path(tokens, "TestApp", "for_module")
        || spells_path(tokens, "TestApp", "builder")
        || spells_path(tokens, "TestApp", "new")
        || spells_path(tokens, "App", "builder")
        || spells_path(tokens, "App", "new")
}

/// Whether a capability's documented wiring is booted anywhere that runs.
///
/// Two homes, and `framework.md` names both: the capability's own `tests/` —
/// "a test in the home crate's `tests/`" — **or `nest-rs-testing` for
/// cross-crate wiring**, which is where the five layer families legitimately
/// live, since a guard pool is only observable through a transport none of them
/// owns. A boot there counts only when the same executed file also names the
/// capability's crate: an app that boots without ever mentioning `nest_rs_pipes`
/// witnesses nothing about pipes.
///
/// Executed, never merely mentioned: `executed_tokens` is what separates a
/// `tests/` target and a `#[cfg(test)]` item from a doc example or a `use`, and
/// the distinction is the one `guards.rs` records having got wrong.
fn boots(root: &Path, krate: &str) -> bool {
    let dir = root.join("crates").join(krate);
    let mut own = rust_files(&dir.join("tests"));
    own.extend(rust_files(&dir.join("src")));
    if own
        .iter()
        .flat_map(|path| executed_tokens(path, root))
        .any(|tokens| is_boot(&tokens))
    {
        return true;
    }

    // Both halves read the **executed** tokens. The naming half was a raw
    // `String::contains` over the file, so a `//!` line or a `// TODO:` naming
    // a capability closed its composition cell — the identical defect this join
    // corrected one column over, where "the `mod resolver;` line in its `lib.rs`
    // filled the `#[resolver]` cell".
    let module = krate.replace('-', "_");
    rust_files(&root.join("crates/nest-rs-testing/tests"))
        .iter()
        .any(|path| {
            let executed = executed_tokens(path, root);
            executed.iter().any(is_boot)
                && executed
                    .iter()
                    .any(|tokens| path_roots(tokens).contains(&module))
        })
}

#[test]
fn every_umbrella_capability_carries_its_witnesses() {
    let root = repo_root();
    let capabilities = capabilities(&root);
    baseline::floor(
        capabilities.len(),
        FLOOR,
        "capability feature(s) in the umbrella manifest",
    );

    let reexports = reexports(&root);
    let installs = documented_installs(&root);
    let readmes = readmes(&root);
    let hygiene = hygiene_attrs(&root);

    let mut holes = BTreeSet::new();
    // A feature that ships a surface without shipping a crate owes one thing:
    // being findable. See [`sub_features`].
    for feature in sub_features(&root, &capabilities) {
        if !installs.contains(&feature) {
            holes.insert(format!("{feature} :: docs ## Install"));
        }
    }
    for cap in &capabilities {
        let concern = cap.concern();
        // The re-export must exist, be `pub`, and be gated on **this**
        // capability's own feature — `nest-rs-core` is the only unconditional
        // one, and it is not a capability.
        let owed = (
            cap.krate.replace('-', "_"),
            concern.clone(),
            Some(cap.feature.clone()),
        );
        if !reexports.contains(&owed) {
            holes.insert(cap.cell("pub use"));
        }
        if !installs.contains(&cap.feature) {
            holes.insert(cap.cell("docs ## Install"));
        }
        let subcrate = format!("cargo add {}", cap.krate);
        if readmes.iter().any(|(_, text)| text.contains(&subcrate)) {
            holes.insert(cap.cell("README installs the sub-crate"));
        }
        // The positive half, on the capability's **own** landing page — the one
        // a crates.io visitor reads. `--dev` is accepted: `nest-rs-testing`
        // installs that way and the rule's point is the umbrella, not the table.
        let own = readmes
            .iter()
            .find(|(krate, _)| *krate == cap.krate)
            .map(|(_, text)| text.as_str())
            .unwrap_or_default();
        let installs_umbrella = own
            .contains(&format!("cargo add nest-rs --features {}", cap.feature))
            || own.contains(&format!(
                "cargo add --dev nest-rs --features {}",
                cap.feature
            ));
        if !installs_umbrella {
            holes.insert(cap.cell("README installs the umbrella"));
        }
        // The composition witness: a boot executed in the capability's own
        // crate. `CLAUDE.md` step 5 — "a test in the capability's **own crate**
        // that boots the documented wiring … and asserts what a caller gets
        // back" — and composition is *executed*, never merely compiled, because
        // a boot also proves the access graph, the resolved config and the
        // mounted routes, which compiling cannot.
        if !boots(&root, &cap.krate) {
            holes.insert(cap.cell("own crate boots the documented wiring"));
        }
    }

    // **The expansion column's population is the decorators, not the
    // capabilities.** `framework.md` states the family that way — "extend it
    // when adding a decorator" — and a capability-keyed walk cannot reach the
    // four every app writes: `nest-rs-core` is an *unconditional* dependency of
    // the umbrella, so it activates no `dep:` feature, is therefore not a
    // capability, and `#[injectable]`, `#[hooks]`, `#[module]` and `#[input]`
    // owed nothing. All four happen to be applied today, which is exactly why
    // the hole was invisible; a fifth added tomorrow would have joined nothing.
    //
    // One cell per decorator, reported under the **crate** that ships it, like
    // every other column — one key scheme per baseline, so a reader is not
    // matching two spellings of the same subject down one file. The surface
    // crate rather than the `*-macros` one, because that is what a reader
    // enables and what the other columns name; `nest-rs-core` for the four the
    // umbrella carries with no feature of its own.
    let owning_crate: BTreeMap<String, String> = capabilities
        .iter()
        .map(|cap| (format!("{}-macros", cap.krate), cap.krate.clone()))
        .collect();
    for dir in crate_dirs() {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with("-macros") {
            continue;
        }
        let owner = owning_crate
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.trim_end_matches("-macros").to_owned());
        for decorator in exported_decorators(&dir) {
            if !hygiene.contains(&decorator) {
                holes.insert(format!("{owner} :: macro-hygiene applies #[{decorator}]"));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        capabilities.len(),
        "capabilities",
        "umbrella capability × witness",
        "a witness the front door owes",
    );
}
