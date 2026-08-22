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
// Naming them is the point: a reader who finds five tests here must not
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
// - **A name collision between the product and the framework.** Inside either
//   workspace the compiler is the check and it is free: two `AuthnModule` in
//   one type namespace is `E0255`, so `cargo check` catches what a join would.
//   The one site it cannot see is `nest-rs-cli/src/templates/*.rs`, where the
//   generated code is a `&str` — and that is exactly where the collision
//   shipped when the `app_` prefix was removed. Its guard is not another join
//   here: it is `nest-rs-cli/tests/e2e/scaffold.rs`, which `cargo check`s a
//   generated workspace. **A template edit is not done until that suite has
//   run** — it is `binary(e2e)`-gated, so the default `not binary(e2e)` run
//   says nothing about it.
// - **"A product module never wears a framework concern's name."** There is no
//   such rule any more, and the join that enforced it is deleted rather than
//   relaxed: the `app_` prefix triggered on the namespace the umbrella
//   re-exports, not on the ident, so it marked fourteen types that collided
//   with nothing. A product name is told from a framework one by the path a
//   caller already types — `io::Result` beside `Result` — and a path is not
//   something a name-shaped join can weigh.
// - **"The project name stops at the workspace."** Checkable in principle,
//   unwritable without the project's name, which no file in this repo declares
//   as data.

/// **The same law read on a file's *content*: a file under `<edge>/` serves
/// that edge and nothing else.**
///
/// `architecture.md` gives an edge folder one job — it is one adapter of one
/// module for one transport — so what the folder says and what the file does are
/// the same statement, exactly as a type's name and its path are. A file that
/// answers two edges from inside one of them makes that statement false, and the
/// cost is never only the path: through 5.1 `nest-rs-authz/src/http/guard.rs`
/// held the `AbilityGuard` that implements `check_http`, `check_graphql`,
/// `check_ws_message` **and** `check_mcp`, so a GraphQL-only app enabled the
/// HTTP feature to reach its own guard, the WS entry compiled under `http`, and
/// three of the demo's four `Authz<Edge>Module`s imported an HTTP adapter they
/// never served. The name read fine; only its location was wrong, which is the
/// class `.claude/skills/architecture/SKILL.md` says an LLM most reliably
/// under-weights — hence a scan rather than a sentence.
///
/// **The vocabulary is derived, never listed.** An edge's dispatch surface is
/// what the framework calls on your type *because of the edge it is bound at*,
/// and it declares itself by name: a `pub trait` whose ident opens with an edge
/// word (`WsGuard`, `McpToolContext`), and a method declared in a `pub trait`
/// carrying one as a `_`-delimited segment (`check_graphql`,
/// `transform_ws_data`). Adding an edge, or a marker for one, therefore extends
/// this test the day it is written.
///
/// **What it cannot see, stated rather than implied:** a trait that is
/// edge-bound but does not *say so* in its name — `SocketContext`,
/// `RouteResponseShaper` — is outside a name-derived law. Deriving the edge from
/// the declaring crate instead would catch them and sweep in every generic
/// utility those crates also export (`Reflector`, `HandlerMetadata`), which is
/// the false-positive trade this shape declines. Nor does it read an **alias**:
/// `pub type AuthzGuard = AbilityGuard<AuthzAbility>;` under `authz/http/` was
/// the product's copy of this same defect, and seeing it needs the aliased
/// type's own file resolved across a crate boundary. Both gaps are owner
/// questions, not holes this test pretends to cover — what closes the alias in
/// practice is that the framework type it points at is now at a root, and a CLI
/// template writes the alias beside it.
///
/// Tests are out of population on purpose: a bridge suite under `graphql/`
/// legitimately declares an HTTP guard fixture, because re-running the HTTP
/// chain in band is the thing under test.
#[test]
fn no_file_under_an_edge_folder_answers_another_edge() {
    let root = repo_root();
    let (markers, entries) = edge_dispatch_vocabulary(&root);
    baseline::floor(markers.len(), 10, "edge dispatch markers");
    baseline::floor(entries.len(), 4, "edge dispatch entries");

    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for area in ["crates", "demo"] {
        for path in nest_rs_conformance::sources::rust_files(&root.join(area)) {
            let segments: Vec<&str> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            if !segments.contains(&"src") {
                continue;
            }
            // The deepest edge folder above the file — `audio/mcp/tool.rs` is
            // mcp's, and a nested one would be read the same way.
            let Some(owner) = segments
                .iter()
                .take(segments.len() - 1)
                .rev()
                .find(|s| crate::units::EDGES.contains(s))
            else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            scanned += 1;

            let mut foreign = BTreeSet::new();
            for item in &ast.items {
                let Item::Impl(block) = item else { continue };
                if let Some((trait_path, _)) = &block.trait_
                    && let Some(last) = trait_path.segments.last()
                    && let Some(edge) = markers.get(&last.ident.to_string())
                    && edge != *owner
                {
                    foreign.insert(format!("impl {} ({edge})", last.ident));
                }
                for sub in &block.items {
                    let syn::ImplItem::Fn(f) = sub else { continue };
                    if let Some(edge) = entries.get(&f.sig.ident.to_string())
                        && edge != *owner
                    {
                        foreign.insert(format!("fn {}() ({edge})", f.sig.ident));
                    }
                }
            }
            if !foreign.is_empty() {
                offenders.push(format!(
                    "{} sits under {owner}/ and answers {}",
                    nest_rs_conformance::sources::relative(&path, &root),
                    foreign.into_iter().collect::<Vec<_>>().join(", "),
                ));
            }
        }
    }

    baseline::floor(scanned, 60, "files under an edge folder");
    assert!(
        offenders.is_empty(),
        "an edge folder says the file serves that edge; answering another from \
         inside it makes the path a false statement, and the fix is the move — \
         a type answering every edge belongs where every edge can reach it, not \
         in the folder of whichever one asked first: {offenders:#?}",
    );
}

/// The edge dispatch surface, read off the framework's own trait declarations.
///
/// Returns `(markers, entries)` — trait idents that open with an edge word, and
/// method idents declared inside a `pub trait` carrying one as a `_`-delimited
/// segment. `__`-prefixed idents are macro seams rather than a surface a
/// developer implements, so they are skipped.
fn edge_dispatch_vocabulary(
    root: &Path,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
) {
    let mut markers = std::collections::BTreeMap::new();
    let mut entries = std::collections::BTreeMap::new();

    for path in nest_rs_conformance::sources::rust_files(&root.join("crates")) {
        let segments: Vec<&str> = path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        if !segments.contains(&"src") {
            continue;
        }
        let Some(ast) = parsed(&path) else { continue };
        for item in &ast.items {
            let Item::Trait(t) = item else { continue };
            if !matches!(t.vis, syn::Visibility::Public(_)) {
                continue;
            }
            let name = t.ident.to_string();
            if let Some(edge) = edge_opening(&name) {
                markers.insert(name, edge);
            }
            for sub in &t.items {
                let syn::TraitItem::Fn(f) = sub else { continue };
                let method = f.sig.ident.to_string();
                if method.starts_with("__") {
                    continue;
                }
                if let Some(edge) = edge_segment(&method) {
                    entries.insert(method, edge);
                }
            }
        }
    }
    (markers, entries)
}

/// The edge a `CamelCase` ident opens with — `WsGuard` is ws's, `Websocket`
/// nobody's, because the word has to end where the next one starts.
fn edge_opening(ident: &str) -> Option<String> {
    crate::units::EDGES
        .iter()
        .find(|edge| {
            let lowered = ident.to_ascii_lowercase();
            lowered.starts_with(*edge)
                && ident[edge.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
        })
        .map(|edge| (*edge).to_string())
}

/// The edge a `snake_case` ident carries as a whole segment — `check_ws_message`
/// is ws's, `http_status` is not a dispatch entry and never reaches here.
fn edge_segment(ident: &str) -> Option<String> {
    crate::units::EDGES
        .iter()
        .find(|edge| ident.split('_').any(|part| part == **edge))
        .map(|edge| (*edge).to_string())
}

/// **The same law, one folder-set wider: a binding folder's files name only its
/// own types.**
///
/// [`no_file_under_an_edge_folder_answers_another_edge`] reads the law over the
/// closed edge vocabulary. This one reads it over the set `architecture.md`
/// describes next: *"A driver gives each port it binds a folder."* A folder under
/// a framework crate's `src/` that declares a `module.rs` **is** such a binding,
/// so the folder is a statement about every file inside it, and a file naming a
/// sibling binding's type makes that statement false.
///
/// **`module.rs` is exempt, and that exemption is the whole precision of the
/// test.** A module composes — naming what it registers is its one job — so
/// `database/module.rs` reaching `http/interceptor.rs` is the layout working.
/// Every *other* file states what it serves. `nest-rs-redis`'s
/// `throttler/store.rs` holding a `RedisQueueConnection` field is the defect
/// this test exists for: three bindings share one Redis handle, and it is named,
/// filed and module-gated for whichever asked first — so enabling the throttler
/// obliges an app with no queue to import `RedisQueueModule` and set
/// `NESTRS_QUEUE__URL`.
///
/// **Framework crates only, and the boundary is not arbitrary.** In
/// `demo/crates/features` the folders under `src/` are *domains*, not bindings —
/// `posts/` depending on `authn/`'s `Claims` is a feature using another feature,
/// which is what a product does. Run there, this scan reports 44 legitimate
/// references and no defect. The product's own sub-folders are already covered
/// one level down, by the edge law above.
///
/// What it cannot see, stated rather than implied: a shared thing whose
/// consumers are in **other crates** — `nest_rs_authn::JwtService`, reached by
/// three OAuth-family crates through a module named for authentication — is the
/// same class across a boundary this scan does not cross. That half is the
/// reviewer's.
#[test]
fn no_binding_folder_names_a_sibling_bindings_type() {
    let root = repo_root();
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;

    // Framework crates only — the boundary the doc above argues. `crate_dirs`
    // walks all three workspaces, and the product's top-level folders are
    // domains rather than bindings.
    let framework = root.join("crates");
    for krate in crate_dirs() {
        if !krate.starts_with(&framework) {
            continue;
        }
        let src = krate.join("src");
        let bindings: BTreeSet<String> = nest_rs_conformance::sources::rust_files(&src)
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "module.rs"))
            .filter_map(|p| p.parent())
            .filter(|d| *d != src)
            .filter_map(|d| d.file_name().and_then(|n| n.to_str()))
            .map(str::to_owned)
            .collect();
        if bindings.len() < 2 {
            continue;
        }

        // Which binding declares each public type. Parsed rather than matched:
        // a `*-macros` crate and a CLI template carry these very idents inside
        // string literals, and a token walk never sees a literal.
        let mut declared: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let mut files = Vec::new();
        for path in nest_rs_conformance::sources::rust_files(&src) {
            let Some(folder) = binding_of(&path, &src, &bindings) else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            for item in &ast.items {
                let (ident, vis) = match item {
                    Item::Struct(i) => (i.ident.to_string(), &i.vis),
                    Item::Enum(i) => (i.ident.to_string(), &i.vis),
                    Item::Type(i) => (i.ident.to_string(), &i.vis),
                    Item::Trait(i) => (i.ident.to_string(), &i.vis),
                    _ => continue,
                };
                if matches!(vis, syn::Visibility::Public(_)) {
                    declared.insert(ident, folder.clone());
                }
            }
            files.push((path, folder, ast));
        }

        for (path, folder, ast) in &files {
            if path.file_name().is_some_and(|n| n == "module.rs") {
                continue;
            }
            scanned += 1;
            let named = idents_in(ast);
            for (ident, home) in &declared {
                if home != folder && named.contains(ident) {
                    holes.insert(format!(
                        "{ident} is {home}/'s, named by {}",
                        nest_rs_conformance::sources::relative(path, &root),
                    ));
                }
            }
        }
    }

    baseline::floor(scanned, 8, "files under a binding folder");
    baseline::gate(
        "bindings-baseline.txt",
        &holes,
        scanned,
        "files under a binding folder",
        "types reached across a binding boundary",
        "a thing several bindings share, named and filed for whichever asked \
         first — it belongs at the level all of them reach, which is the crate \
         root",
    );
}

/// The binding folder a path sits under, or `None` when it sits at the crate
/// root or under something that declares no module.
fn binding_of(path: &Path, src: &Path, bindings: &BTreeSet<String>) -> Option<String> {
    let rest = path.strip_prefix(src).ok()?;
    let first = rest.components().next()?.as_os_str().to_str()?;
    bindings.contains(first).then(|| first.to_owned())
}

/// Every identifier the file's tokens carry. A token walk rather than a text
/// scan, so a name inside a string literal — which is all a CLI template holds —
/// is not a reference.
fn idents_in(file: &syn::File) -> BTreeSet<String> {
    fn walk(stream: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
        for tree in stream {
            match tree {
                proc_macro2::TokenTree::Ident(i) => {
                    out.insert(i.to_string());
                }
                proc_macro2::TokenTree::Group(g) => walk(g.stream(), out),
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(quote::ToTokens::to_token_stream(file), &mut out);
    out
}
