//! The naming join: `architecture.md`'s layout law, against the tree.
//!
//! That file is the repo's one copy of the naming rules — `CLAUDE.md` calls it
//! "one copy, not a description of one" — and every sentence in it was prose
//! nothing ran. This module runs the ones that are a path or a symbol, and
//! **names the ones that are not** at the bottom, because *not mechanisable* and
//! *not yet done* are different sentences and only one of them is an excuse.
//!
//! Members are derived from the source in every case: the crate directories
//! themselves, the folders under each `src/`, and — for the reserved
//! vocabulary — the block inside `architecture.md`, parsed rather than
//! recopied. A second copy of that list here would be the defect the file
//! exists to prevent.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{crate_dirs, parsed, repo_root};
use syn::Item;

const BASELINE: &str = "naming-baseline.txt";

/// Below this the scan is reading the wrong tree and every agreement it reports
/// is an artefact.
const FLOOR: usize = 18;

/// Fold a name to what it *says*, discarding how it was cased or separated:
/// `oauth-client`, `OAuthClient` and `Oauth_Client` all fold to `oauthclient`.
///
/// Folding rather than comparing literally is what lets the repo keep writing
/// one word two ways where English does — `openapi`/`OpenApi`,
/// `opentelemetry`/`OpenTelemetry`, `server-timing`/`ServerTiming` all agree
/// here and none would survive `==`.
fn folded(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The DI types a parsed `module.rs` declares — the module and the seam types
/// that travel with it. All three share the stem; checking only the `*Module`
/// is how a `RedisThrottlerModule` came to sit beside a `ThrottlerSetup`.
fn declared_module_types(file: &syn::File) -> Vec<String> {
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(s) => Some(s.ident.to_string()),
            Item::Type(t) => Some(t.ident.to_string()),
            _ => None,
        })
        .filter(|n| n.ends_with("Module") || n.ends_with("Setup") || n.ends_with("Host"))
        .collect()
}

/// A crate's own subject — what follows the workspace prefix, which names the
/// workspace and nothing about this crate.
fn subject_of(krate: &str) -> &str {
    krate.strip_prefix("nest-rs-").unwrap_or(krate)
}

/// **The law: the stem is the crate's subject plus every folder below `src/`.**
///
/// `CLAUDE.md` calls naming the pillar, and this is the mechanical half of it:
/// from a path you know the type, from a type you know the path. `redis/queue/`
/// is `RedisQueue*`, `seaorm/health/` is `SeaOrmHealth*`, a crate whose
/// `module.rs` sits at its root is its own subject.
///
/// **A port keeps the bare name; a driver carries its own.** `ThrottlerStore`'s
/// implementations were already `InMemoryThrottler` and `RedisThrottler`, so the
/// module follows the implementation — `nest_rs::redis::RedisThrottlerModule`
/// sits beside `RedisThrottler` and says the same thing, while the bare
/// `ThrottlerModule` belongs to the crate defining the port. The path stutters
/// and a backend swap edits the type name; both are paid on purpose, because a
/// name that is unambiguous in a stack trace outranks a name that is short in an
/// import, and a module name lives in a composition root rather than in fifty
/// call sites.
///
/// **Every type in the file shares the stem**, not just the `*Module` — a rename
/// that leaves its `*Setup` or `*Host` behind is half a rename, and the half
/// left behind is the one a reader trips on. The stem is a **prefix**, not an
/// equality, so a file declaring two of one role may qualify them:
/// `ConfigRootSetup` and `ConfigFeatureSetup` name two seams and both carry
/// `Config`.
///
/// A product library is a container rather than a subject, so `features` never
/// prefixes: `audio/http/module.rs` is `AudioHttpModule`.
#[test]
fn every_module_type_is_named_for_its_path() {
    /// Words whose PascalCase is not a mechanical uppercase of the first letter.
    ///
    /// Consulted **per `-`-separated segment**, so a family declares its
    /// spelling once and every member inherits it: `oauth` here is what makes
    /// `oauth-client`, `oauth-server` and `oauth-resource` derive `OAuthClient`,
    /// `OAuthServer` and `OAuthResource` without a line each. A role added to
    /// the family tomorrow needs no edit here.
    const SPELLED: [(&str, &str); 6] = [
        ("oauth", "OAuth"),
        ("seaorm", "SeaOrm"),
        ("openapi", "OpenApi"),
        ("opentelemetry", "OpenTelemetry"),
        ("macro-hygiene", "MacroHygiene"),
        ("graphql", "Graphql"),
    ];
    fn pascal(s: &str) -> String {
        s.split(['-', '_'])
            .map(|w| match SPELLED.iter().find(|(k, _)| *k == w) {
                Some((_, spelled)) => (*spelled).to_owned(),
                None => {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                        None => String::new(),
                    }
                }
            })
            .collect()
    }

    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    let root = repo_root();

    for dir in crate_dirs() {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A framework crate is named for its subject, so it prefixes. A product
        // library is a container — its modules are domains, and
        // `FeaturesAudioHttpModule` would name the container nobody thinks in.
        let base = match krate.strip_prefix("nest-rs-") {
            Some(subject) => SPELLED
                .iter()
                .find(|(k, _)| *k == subject)
                .map(|(_, v)| (*v).to_owned())
                .unwrap_or_else(|| pascal(subject)),
            None => String::new(),
        };

        let src = dir.join("src");
        for path in nest_rs_conformance::sources::rust_files(&src) {
            if path.file_name().is_some_and(|n| n != "module.rs") {
                continue;
            }
            let Some(ast) = parsed(&path) else { continue };
            let folders: String = path
                .strip_prefix(&src)
                .ok()
                .and_then(|rel| rel.parent())
                .map(|p| {
                    p.components()
                        .filter_map(|c| c.as_os_str().to_str())
                        .map(|seg| {
                            SPELLED
                                .iter()
                                .find(|(k, _)| *k == seg)
                                .map(|(_, v)| (*v).to_owned())
                                .unwrap_or_else(|| pascal(seg))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // A framework crate with no folder is its own stem; a product
            // module with no folder would have none, and there are none.
            let stem = if base.is_empty() && folders.is_empty() {
                pascal(subject_of(krate))
            } else {
                format!("{base}{folders}")
            };

            for name in declared_module_types(&ast) {
                scanned += 1;
                if !name.starts_with(&stem) {
                    holes.insert(format!(
                        "{name}  ({})  — expected `{stem}…`",
                        nest_rs_conformance::sources::relative(&path, &root),
                    ));
                }
            }
        }
    }

    baseline::floor(scanned, FLOOR, "module types");
    baseline::gate(
        BASELINE,
        &holes,
        scanned,
        "module types",
        "module types whose name does not carry their path",
        "a type a reader cannot locate from its name, or name from its location. \
         The fix is the rename, or the folder the file should have been in — never \
         a line here",
    );
}

/// **The same law one level down: an edge adapter is named for its module.**
///
/// `architecture.md` spells the shape as `<Module><Edge>Module` for the module
/// and the role tables put the adapter's own types beside it, so
/// `posts/http/controller.rs` is `PostsController` and `users/ws/gateway.rs` is
/// `UsersGateway`. The **module** owns the name, not the edge folder: an edge is
/// an adapter *of* something, and `HttpController` would name the adapter twice
/// and the thing it adapts never.
#[test]
fn every_edge_adapter_is_named_for_the_module_it_adapts() {
    const ROLES: [(&str, &str); 5] = [
        ("controller.rs", "Controller"),
        ("resolver.rs", "Resolver"),
        ("gateway.rs", "Gateway"),
        ("processor.rs", "Processor"),
        ("listener.rs", "Listener"),
    ];
    let root = repo_root();
    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for area in ["crates", "demo"] {
        for path in nest_rs_conformance::sources::rust_files(&root.join(area)) {
            let path = path.as_path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((_, role)) = ROLES.iter().find(|(f, _)| *f == name) else {
                continue;
            };
            let segments: Vec<&str> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            // …/<module>/<edge>/<role>.rs — the edge folder is what marks this
            // file as an adapter rather than a module-root role file.
            let Some(edge_at) = segments.len().checked_sub(2) else {
                continue;
            };
            if !crate::units::EDGES.contains(&segments[edge_at]) || edge_at == 0 {
                continue;
            }
            let module = folded(segments[edge_at - 1]);
            let Some(ast) = parsed(path) else { continue };
            for item in &ast.items {
                let syn::Item::Struct(s) = item else { continue };
                let ident = s.ident.to_string();
                if !ident.ends_with(role) {
                    continue;
                }
                scanned += 1;
                if !folded(&ident).starts_with(&module) {
                    offenders.push(format!(
                        "{ident} in {}",
                        nest_rs_conformance::sources::relative(path, &root),
                    ));
                }
            }
        }
    }

    baseline::floor(scanned, 12, "edge adapters");
    assert!(
        offenders.is_empty(),
        "an adapter is named for the module it adapts, so a reader finds it from \
         the feature they are working on rather than from the transport: \
         {offenders:#?}",
    );
}

/// `architecture.md`: "**No `*_module.rs`, ever.** One `#[module]` per file, one
/// `module.rs` per folder; two modules in a feature means two folders."
///
/// No baseline: the tree is clean today, and the whole value is that it stays
/// so. A first offender should fail the build, not land a line.
#[test]
fn no_file_is_named_for_a_module_instead_of_being_one() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for area in ["crates", "demo"] {
        for path in nest_rs_conformance::sources::rust_files(&root.join(area)) {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_module.rs"))
            {
                offenders.push(nest_rs_conformance::sources::relative(&path, &root));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`architecture.md` closes this one absolutely — a DI module is \
         `module.rs` in its own folder, never `<name>_module.rs`: {offenders:#?}",
    );
}

/// `architecture.md`: "A folder invented to group 'things that go together' —
/// `contract/`, `types/`, `core/`, `shared/`, `common/`, `interfaces/` — is a
/// defect: every file it would hold is already named by a table above […] A
/// folder that feels too full means the module is too big; split the module,
/// never the vocabulary."
///
/// The six names are the ones the rule itself enumerates, so this list is a
/// quotation rather than an invention.
#[test]
fn no_module_invents_a_folder_to_group_things_that_go_together() {
    const INVENTED: [&str; 6] = [
        "contract",
        "types",
        "core",
        "shared",
        "common",
        "interfaces",
    ];
    let root = repo_root();
    let mut offenders = Vec::new();

    for area in ["crates", "demo"] {
        walk_dirs(&root.join(area), &mut |dir| {
            // Only folders *inside* a `src/` tree are module vocabulary; a
            // crate may legitimately be called `nest-rs-core`.
            if !dir.components().any(|c| c.as_os_str() == "src") {
                return;
            }
            if dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| INVENTED.contains(&n))
            {
                offenders.push(nest_rs_conformance::sources::relative(dir, &root));
            }
        });
    }
    assert!(
        offenders.is_empty(),
        "each of these is a bag, and every file in it is already named by a role \
         table — split the module instead: {offenders:#?}",
    );
}

/// `architecture.md`: "**A module may not take a name from the structural
/// vocabulary.** These words already mean something to the layout, and reusing
/// one makes every path ambiguous. Pick the domain word instead — a module about
/// desktop applications is `programs`, not `apps`."
///
/// **Level-aware, and it has to be.** The same word is legal one level down: a
/// module's `http/` is the sanctioned edge folder and its `dtos/` the sanctioned
/// plural role folder. What the rule forbids is a *module* — a domain — taking
/// one. So the population is the folders directly under a product `src/`, which
/// is where a module lives.
///
/// The reserved list is parsed out of `architecture.md` itself. Recopying it
/// here would be the second copy `CLAUDE.md` says that file exists to prevent.
#[test]
fn no_module_takes_a_name_from_the_structural_vocabulary() {
    let root = repo_root();
    let reserved = reserved_vocabulary(&root);
    assert!(
        reserved.len() > 20,
        "parsed {} reserved words out of architecture.md — the block moved and \
         this test is now reading nothing",
        reserved.len(),
    );

    let mut offenders = Vec::new();
    let mut module_roots = vec![root.join("demo/crates/features/src")];
    if let Ok(apps) = std::fs::read_dir(root.join("demo/apps")) {
        module_roots.extend(apps.flatten().map(|a| a.path().join("src")));
    }

    for src in module_roots {
        let Ok(entries) = std::fs::read_dir(&src) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if reserved.contains(&name) {
                offenders.push(nest_rs_conformance::sources::relative(&entry.path(), &root));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a module named from the layout's own vocabulary makes every path below \
         it ambiguous — pick the domain word: {offenders:#?}",
    );
}

/// The `reserved_block` of `architecture.md`, split into words.
///
/// Read from `crates/nest-rs-cli/src/templates/architecture.md`, which is the
/// **real file** — `.claude/rules/architecture.md` is a symlink to it, and the
/// CLI `include_str!`s it into every scaffolded project.
fn reserved_vocabulary(root: &Path) -> BTreeSet<String> {
    let path = root.join("crates/nest-rs-cli/src/templates/architecture.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    let Some(after) = text.split("## Reserved vocabulary").nth(1) else {
        return BTreeSet::new();
    };
    let Some(block) = after.split("```").nth(1) else {
        return BTreeSet::new();
    };
    block
        .split_whitespace()
        // The block is a table: each line opens with its category
        // (`structure`, `roles`, `plurals`, `edges`), which is a label rather
        // than a reserved word.
        .filter(|w| !["structure", "roles", "plurals", "edges"].contains(w))
        .map(str::to_owned)
        .collect()
}

/// Every directory under `dir`, recursively, skipping build output.
fn walk_dirs(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || path.file_name().is_some_and(|n| n == "target") {
            continue;
        }
        visit(&path);
        walk_dirs(&path, visit);
    }
}

// ── What this join deliberately does not check, and why ─────────────────────
//
// Naming them is the point: a reader who finds four tests here must not
// conclude the file's law is four sentences long.
//
// - **Which declared subject is the right one.** The law binds the vocabulary,
//   not the choice: a module may name itself for its crate's subject, a port it
//   depends on, or its folder, and this join cannot say which of those the
//   author should have picked. `CoreModule` inside `nest-rs-http` segments
//   cleanly and is still a bad name. What it does catch is the class that
//   actually shipped — twice — a subject **nothing declares**, which is what a
//   synonym split always is: `DiscoveryModule` under `nest-rs-oauth-resource`
//   fails here, and that is the defect this join was written for. Adjudicating
//   between two declared subjects stays `/architecture`'s, and it is judgement

/// **"A product module whose name is one the umbrella re-exports takes the
/// `app_` prefix."** — `architecture.md`, *The product's own*.
///
/// The defect is invisible at the call site, which is why it needs a join.
/// `features::authn::AuthnModule` and `nest_rs::authn::AuthnModule` are written
/// `AuthnModule` in both `imports` lists, so the reader has to go back to the
/// `use` line to learn which module they are looking at — the one property the
/// naming law exists to buy, lost.
///
/// **Both populations are derived.** The reserved set is the umbrella's own
/// re-exports, root-level and inside each family module, read out of
/// `crates/nest-rs/src/lib.rs` — so a capability shipped tomorrow reserves its
/// word here without an edit. The candidates are the module folders of every
/// crate that is not a `nest-rs-*`, which is what "a product module" means.
/// A word outside the set never takes the prefix: `users` and `posts` collide
/// with nothing, and `app_users` would be noise.
#[test]
fn no_product_module_takes_a_framework_concern_without_the_app_prefix() {
    let root = repo_root();
    let mut reserved = BTreeSet::new();
    if let Some(ast) = parsed(&root.join("crates/nest-rs/src/lib.rs")) {
        // A concern is re-exported at the root (`pub use nest_rs_http as http;`)
        // or one level down inside its family (`pub mod oauth { pub use
        // nest_rs_oauth_client as client; }`). Families are one level by rule,
        // so both the family's own word and each member's are reserved.
        let mut collect = |item: &Item, family: Option<&str>| {
            if let Item::Use(use_item) = item
                && let syn::UseTree::Rename(rename) = &use_item.tree
                && matches!(use_item.vis, syn::Visibility::Public(_))
            {
                reserved.insert(rename.rename.to_string());
                if let Some(name) = family {
                    reserved.insert(name.to_owned());
                }
            }
        };
        for item in &ast.items {
            match item {
                Item::Mod(module) => {
                    let name = module.ident.to_string();
                    if let Some((_, inner)) = &module.content {
                        for nested in inner {
                            collect(nested, Some(&name));
                        }
                    }
                }
                other => collect(other, None),
            }
        }
    }

    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    for dir in crate_dirs() {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if krate.starts_with("nest-rs-") || krate == "nest-rs" {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir.join("src")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(module) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            scanned += 1;
            if reserved.contains(module) {
                holes.insert(format!(
                    "{module}  ({krate}/src/{module}/)  — the umbrella re-exports \
                     `nest_rs::{module}`; expected `app_{module}`"
                ));
            }
        }
    }

    baseline::floor(scanned, 8, "product modules");
    baseline::gate(
        BASELINE,
        &holes,
        scanned,
        "product modules",
        "product module(s) wearing a framework concern's name",
        "a type a reader cannot tell from the framework's own without going back \
         to the `use` line. The fix is the `app_` prefix on the folder, and every \
         type in it follows — never a line here",
    );
}

//   rather than a scan.
// - **"One `#[module]` per file."** Counting the attribute is a false-positive
//   machine, and measuring it proved so: `src/module.rs` files hold 0, 2, 3, 4
//   and 7 of them today and every one is legal. Zero means a hand-written
//   `impl Module`, which `framework.md` says "is still a DI module"; the higher
//   counts are `#[cfg(test)]` fixtures and doctests inside the same file. A
//   sound check needs to parse, drop test-gated items and doctests, and then
//   still reconcile the hand-written form — which is a different test from this
//   one and owes its own argument.
// - **"A file exists only if it has real content."** *Real* is the judgement;
//   an empty `mod.rs` is legal and a one-line one may or may not be.
// - **The role tables** (`service.rs`, `guard.rs`, `controller.rs`, …). What a
//   file *is* comes from what it holds, so checking the name against the role
//   means classifying the contents — `/architecture`'s job, not a join's.
// - **"The project name stops at the workspace."** Checkable in principle,
//   unwritable without the project's name, which no file in this repo declares
//   as data.
