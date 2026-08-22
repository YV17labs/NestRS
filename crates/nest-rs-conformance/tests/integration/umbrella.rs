//! The umbrella join: every capability the front door offers, against the
//! witnesses `CLAUDE.md` makes it owe.
//!
//! *The umbrella is the front door* fixes what shipping a capability costs — a
//! feature in the matrix, a `pub use`, `cargo add nest-rs --features <x>` in the
//! README **and** the docs page's `## Install`, and two witnesses. It reads as a
//! checklist and was checked by nobody, so a capability could reach a user with
//! no documented way to install it.
//!
//! A **capability** is derived, not listed — and the word names two populations
//! that this file used to conflate, so it now says which one it means.
//!
//! `sources::UmbrellaMatrix` holds the split. A **feature** is what a developer
//! types after `--features`; there are twenty-eight, `default` and `full` aside,
//! and that is the set the landing counts and the packages page maps. A
//! **crate** is what a feature's `dep:` entry activates; there are also
//! twenty-eight, but not the same twenty-eight — `seaorm` activates two
//! (`#[expose]` expands to one, `#[crud]` to the other) and `redis-throttler`
//! activates none.
//!
//! **This join's rows are crates**, because what it asks is what each crate
//! owes: a re-export, an install line, a README, an expansion witness, a boot.
//! Its columns key on the crate for the reason [`Capability::cell`] argues.
//! Until this paragraph existed the doc here said *feature* while the code
//! counted `dep:` entries, and the numbers agreed only for as long as every
//! capability feature activated exactly one crate. The day that stopped being
//! true the docs linter — which had copied the sentence, not the code — began
//! publishing 27 against a landing that correctly said 28.
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
    UMBRELLA_AGGREGATES, crate_dirs, executed_tokens, exported_decorators, files_with_extension,
    parsed, path_roots, read, repo_root, rust_files, spells_path, umbrella_matrix,
};
use syn::{Item, UseTree};

const BASELINE: &str = "umbrella-baseline.txt";

/// Twenty-eight crates stand behind the matrix today. Below that the scan is
/// reading the wrong manifest and every hole it reports is an artefact.
///
/// **This counts crates, and the word is now spelled where it is defined.** The
/// module doc above used to define a capability as *a feature* while this
/// number counted `dep:` **entries** — 27 of the first, 28 of the second, since
/// `seaorm` activates two crates. One word, two populations, and the docs linter
/// picked the other one: it published 27 against a landing that said 28.
/// `sources::UmbrellaMatrix` now names both and this join takes the crate view,
/// which is its own subject — see [`Capability::cell`].
const CRATE_FLOOR: usize = 28;

/// The README corpus is at least the size of the crate list, and that is its
/// own floor rather than a borrowed one.
///
/// It read [`CRATE_FLOOR`] until this line existed: two unrelated populations
/// behind one constant, so dropping to twenty capabilities would have loosened
/// a README check to twenty for no reason — weakening exactly the column whose
/// stated purpose is that a negative check over an empty corpus reports nothing.
/// `baseline::floor` makes the same argument for why the *sentence* is central
/// and the *number* is not.
const README_FLOOR: usize = 28;

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
    /// The path `::nest_rs::<concern>::` a macro expansion resolves through.
    ///
    /// One segment for a crate that stands alone, two for a member of a
    /// **family** — `nest-rs-oauth-client` is reached at `oauth::client`,
    /// because the umbrella groups RFC 6749's roles under one module. The
    /// families are read off the umbrella rather than listed here, so adding a
    /// role is one `pub use` and no edit to this join.
    fn concern(&self, families: &BTreeSet<String>) -> String {
        let subject = self.krate.strip_prefix("nest-rs-").unwrap_or(&self.krate);
        match subject.split_once('-') {
            Some((family, member)) if families.contains(family) => {
                format!("{family}::{}", member.replace('-', "_"))
            }
            _ => subject.replace('-', "_"),
        }
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
        .filter(|name| !UMBRELLA_AGGREGATES.contains(&name.as_str()))
        .collect()
}

/// Every crate the umbrella activates, one row per `dep:` entry.
///
/// Reads `sources::umbrella_matrix`, which is where the `[features]` table is
/// parsed for the whole crate — this join and the `canon` join both need it, and
/// two parsers for one table is how the two answers came to differ. The *view*
/// is this join's own: it asks what each crate owes, so a crate is what a row
/// names, for the reason [`Capability::cell`] argues.
fn capabilities(root: &Path) -> Vec<Capability> {
    umbrella_matrix(root)
        .crates()
        .into_iter()
        .map(|(feature, krate)| Capability { feature, krate })
        .collect()
}

/// One umbrella re-export: the crate it renames, the concern path it exposes it
/// at, and the feature gating it.
type Reexport = (String, String, Option<String>);

/// The concerns the umbrella re-exports, as `<crate> as <concern>`, plus the
/// **family** modules it declares.
///
/// A capability is re-exported at the root (`pub use nest_rs_http as http;`) or
/// one level down inside its family (`pub mod oauth { pub use
/// nest_rs_oauth_client as client; }`). Families are one level by rule, so one
/// level of descent is the whole search — and the module names it finds are
/// what [`Capability::concern`] reads, which is why the family is declared once,
/// in the umbrella, and nowhere else.
fn reexports(root: &Path) -> (BTreeSet<Reexport>, BTreeSet<String>) {
    let Some(ast) = parsed(&root.join("crates/nest-rs/src/lib.rs")) else {
        return (BTreeSet::new(), BTreeSet::new());
    };
    let mut families = BTreeSet::new();
    let flattened: Vec<(Option<String>, &Item)> = ast
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Mod(module) => module
                .content
                .as_ref()
                .map(|(_, inner)| {
                    let name = module.ident.to_string();
                    inner
                        .iter()
                        .map(|nested| (Some(name.clone()), nested))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            other => vec![(None, other)],
        })
        .collect();
    let found = flattened
        .into_iter()
        .filter_map(|(family, item)| {
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
            let alias = rename.rename.to_string();
            let concern = match &family {
                Some(name) => {
                    families.insert(name.clone());
                    format!("{name}::{alias}")
                }
                None => alias,
            };
            Some((rename.ident.to_string(), concern, gate))
        })
        .collect();
    (found, families)
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
        out.len() > README_FLOOR,
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
        CRATE_FLOOR,
        "crate(s) activated by the umbrella manifest",
    );

    let (reexports, families) = reexports(&root);
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
        let concern = cap.concern(&families);
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

/// **An edge the umbrella turns on for the guard trait, it turns on for every
/// crate that implements that edge's entry.**
///
/// `Guard::check_graphql` / `check_ws_message` / `check_mcp` each have a default
/// `Ok(())` body, so a guard whose arm is compiled out **authorises everything
/// at that edge, silently**. Two optional crates implement those arms behind
/// their own per-edge features, and the umbrella pairs them by hand:
/// `ws = [.., "nest-rs-guards/ws", "nest-rs-authz?/ws", "nest-rs-throttler?/ws"]`.
///
/// Nothing held that pairing until this test. It used to be structural —
/// `nest-rs-authz`'s `http` feature forwarded `nest-rs-guards/ws`, so the WS arm
/// existed whenever the guard did — and moving `AbilityGuard` to the crate root
/// in 5.2 replaced that with the hand-written lines above. An audit reproduced
/// the gap the day it opened: same source, `authz = ["http"]` with
/// `guards = ["ws"]`, a message with no ambient ability came back **PASSED**
/// where `HEAD` returned **DENIED 401**. Unreachable through the umbrella, and
/// unreachable is exactly what this test is for.
///
/// The population is derived, never listed: a crate implementing `fn check_<x>`
/// owes the pairing, so a new guard crate joins the day it is written.
#[test]
fn every_edge_the_umbrella_arms_is_armed_on_every_guard_crate() {
    // `Guard`'s per-edge entries and the umbrella feature each belongs to.
    const ENTRIES: [(&str, &str); 3] = [
        ("check_graphql", "graphql"),
        ("check_ws_message", "ws"),
        ("check_mcp", "mcp"),
    ];
    let root = repo_root();
    let matrix = nest_rs_conformance::sources::umbrella_matrix(&root);
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for (entry, edge) in ENTRIES {
        // Who implements this entry, read off the tree. `nest-rs-guards` owns
        // the trait rather than implementing it for a bound guard, and
        // `nest-rs-macro-hygiene` is the one-dependency witness crate — neither
        // is an optional crate the umbrella pairs.
        let implementors: BTreeSet<String> = nest_rs_conformance::sources::crate_dirs()
            .into_iter()
            .filter(|k| k.starts_with(root.join("crates")))
            .filter(|k| {
                nest_rs_conformance::sources::rust_files(&k.join("src"))
                    .iter()
                    .filter_map(|p| nest_rs_conformance::sources::parsed(p))
                    .any(|f| file_declares_impl_fn(&f, entry))
            })
            .filter_map(|k| k.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .filter(|k| k != "nest-rs-guards" && k != "nest-rs-macro-hygiene")
            .collect();

        let enabled = matrix.entries_of(edge);
        if enabled.is_empty() {
            offenders.push(format!("the umbrella declares no `{edge}` feature"));
            continue;
        }
        for krate in &implementors {
            checked += 1;
            let pairing = format!("{krate}?/{edge}");
            if !enabled.iter().any(|e| e == &pairing) {
                offenders.push(format!(
                    "`{edge}` arms `{entry}` but does not enable `{pairing}` — \
                     that crate's guard would authorise every {edge} unit in silence",
                ));
            }
        }
    }

    baseline::floor(checked, 4, "edge/guard-crate pairings");
    assert!(
        offenders.is_empty(),
        "every `Guard::check_*` entry defaults to `Ok(())`, so an arm the \
         umbrella leaves compiled out is a guard that passes rather than one \
         that is absent: {offenders:#?}",
    );
}

/// Whether the file carries an `impl` block declaring a method of this name —
/// the shape that *answers* an edge, as opposed to the trait that declares it.
fn file_declares_impl_fn(file: &syn::File, method: &str) -> bool {
    file.items.iter().any(|item| {
        matches!(item, Item::Impl(block) if block.items.iter().any(|sub| {
            matches!(sub, syn::ImplItem::Fn(f) if f.sig.ident == method)
        }))
    })
}
