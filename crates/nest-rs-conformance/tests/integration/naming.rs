//! The naming law, where it is a function of the path.
//!
//! `architecture.md`: *a name and its path say the same thing*. Most of that law
//! is mechanical — the stem is the crate's subject plus every folder below
//! `src/` — so it is checked here against the tree: module types, edge
//! adapters and `#[config]` namespaces against their stem, and the layout laws
//! that are only paths (`module.rs` is the DI module, no `*_module.rs`, no bag
//! folders, no module named from the structural vocabulary, no edge adapter at
//! a product root).
//!
//! What it reads in a file is the declaration a stem names — a struct, a
//! `#[module]`, a `#[config]` — by its attribute's last segment. A decorator
//! renamed on import (`use nest_rs::module as m`) is not seen, and that is
//! accepted: the rule is the review's as much as this file's.
//!
//! The judgement half of the law is review, never a scan here: which declared
//! subject is the right one, whether a file under `<edge>/` answers another
//! edge, whether a binding names a sibling binding's type, whether a root file
//! of an adapter crate serves several bindings, whether an error type lives in
//! `error.rs`, whether a `*Config` is a `#[config]`.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nest_rs_conformance::sources::{
    config_namespace, crate_dirs, declared_targets, is_cfg_test, item_attrs, parsed, relative,
    repo_root, rust_files, segments,
};
use syn::Item;
use syn::visit::Visit;

/// The closed edge vocabulary (`architecture.md`). Stated rather than derived:
/// it is closed by an owner decision, so opening an edge touches the rules and
/// this line, which is the reviewed act the closure exists to require.
const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];

/// The folder the product lives under, from the repository root.
const PRODUCT: &str = "demo";

/// Where the product's crates sit below [`PRODUCT`]: its binaries, and the
/// libraries they share.
const PRODUCT_WORKSPACES: [&str; 2] = ["apps", "crates"];

/// Words whose PascalCase is not a mechanical uppercase of the first letter,
/// consulted per `-`-separated segment so a family declares its spelling once.
const SPELLED: [(&str, &str); 6] = [
    ("oauth", "OAuth"),
    ("seaorm", "SeaOrm"),
    ("openapi", "OpenApi"),
    ("opentelemetry", "OpenTelemetry"),
    ("macro-hygiene", "MacroHygiene"),
    ("graphql", "Graphql"),
];

/// Fold a name to what it *says*: `oauth-client`, `OAuthClient` and
/// `Oauth_Client` all fold to `oauthclient`.
fn folded(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A crate's own subject — what follows the workspace prefix.
fn subject_of(krate: &str) -> &str {
    krate.strip_prefix("nest-rs-").unwrap_or(krate)
}

/// A folder or crate word as the type that names it spells it.
fn pascal(s: &str) -> String {
    if let Some((_, spelled)) = SPELLED.iter().find(|(k, _)| *k == s) {
        return (*spelled).to_owned();
    }
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

fn crate_name(dir: &Path) -> &str {
    dir.file_name().and_then(|n| n.to_str()).unwrap_or_default()
}

fn is_macros(dir: &Path) -> bool {
    crate_name(dir).ends_with("-macros")
}

/// Whether an attribute list carries a decorator whose path ends in `name`.
fn decorated(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs
        .iter()
        .any(|attr| attr.path().segments.last().is_some_and(|s| s.ident == name))
}

/// Every struct, enum, union and alias an item list declares, inline modules
/// included and `#[cfg(test)]` ones left out.
fn declared_types(items: &[Item], out: &mut Vec<String>) {
    for item in items {
        if is_cfg_test(item_attrs(item)) {
            continue;
        }
        match item {
            Item::Struct(i) => out.push(i.ident.to_string()),
            Item::Enum(i) => out.push(i.ident.to_string()),
            Item::Union(i) => out.push(i.ident.to_string()),
            Item::Type(i) => out.push(i.ident.to_string()),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    declared_types(inner, out);
                }
            }
            _ => {}
        }
    }
}

/// **Every type a `module.rs` declares carries the stem** — the crate's subject
/// plus every folder below `src/`: `redis/queue/module.rs` is `RedisQueue*`,
/// its `*Setup` and `*Host` included, since a rename that leaves a sibling
/// behind is half a rename. The stem is a prefix, so a file declaring two of
/// one role may qualify them (`ConfigRootSetup`, `ConfigFeatureSetup`). A
/// product library is a container, so `features` never prefixes:
/// `audio/http/module.rs` is `AudioHttpModule`.
#[test]
fn every_module_type_is_named_for_its_path() {
    let root = repo_root();
    let mut holes = Vec::new();
    let mut scanned = 0usize;

    for dir in crate_dirs() {
        // A proc-macro crate's `module.rs` is the `#[module]` decorator's own
        // implementation: a DI module is what it emits, never what it holds.
        if is_macros(&dir) {
            continue;
        }
        let krate = crate_name(&dir);
        let base = krate
            .strip_prefix("nest-rs-")
            .map(pascal)
            .unwrap_or_default();
        let src = dir.join("src");
        for path in rust_files(&src) {
            if path.file_name().is_some_and(|n| n != "module.rs") {
                continue;
            }
            let Some(ast) = parsed(&path) else { continue };
            let parts = segments(&path, &src);
            let folders: String = parts[..parts.len() - 1].iter().map(|s| pascal(s)).collect();
            let stem = if base.is_empty() && folders.is_empty() {
                pascal(subject_of(krate))
            } else {
                format!("{base}{folders}")
            };
            let mut types = Vec::new();
            declared_types(&ast.items, &mut types);
            for name in types {
                scanned += 1;
                if !name.starts_with(&stem) {
                    holes.push(format!(
                        "{name}  ({})  — expected `{stem}…`",
                        relative(&path, &root)
                    ));
                }
            }
        }
    }

    assert!(
        scanned >= 18,
        "read {scanned} module type(s), expected at least 18"
    );
    holes.sort();
    assert!(
        holes.is_empty(),
        "a type a reader cannot locate from its name, or name from its location — \
         rename it, or move the file to the folder it names:\n  {}",
        holes.join("\n  "),
    );
}

/// `#[module]` structs and `impl Module` blocks — modules — and `impl
/// DynamicModule` blocks — the `*Setup`s `for_root` returns — outside
/// `#[cfg(test)]`.
#[derive(Default)]
struct DiModules {
    modules: usize,
    setups: usize,
}

impl<'ast> Visit<'ast> for DiModules {
    fn visit_item(&mut self, node: &'ast Item) {
        if !is_cfg_test(item_attrs(node)) {
            syn::visit::visit_item(self, node);
        }
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if decorated(&node.attrs, "module") {
            self.modules += 1;
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let implemented = node
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .map(|segment| segment.ident.to_string());
        match implemented.as_deref() {
            Some("Module") => self.modules += 1,
            Some("DynamicModule") => self.setups += 1,
            _ => {}
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// **A DI module is declared in a `module.rs`, and only there** — one per file,
/// since two modules in a feature are two folders. A `*Setup` is half of a
/// module and sits beside it; a module offering `for_root` and `for_feature`
/// returns two from one file, so setups are not counted toward the one.
#[test]
fn every_di_module_is_declared_in_a_module_rs() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for dir in crate_dirs().into_iter().filter(|dir| !is_macros(dir)) {
        for path in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            let mut found = DiModules::default();
            found.visit_file(&ast);
            let at = relative(&path, &root);
            let here = path.file_name().is_some_and(|n| n == "module.rs");
            if !here && found.modules > 0 {
                offenders.push(format!("{at} declares a DI module"));
            }
            if !here && found.setups > 0 {
                offenders.push(format!(
                    "{at} implements `DynamicModule` — a `*Setup` sits beside its module"
                ));
            }
            if here && found.modules > 1 {
                offenders.push(format!(
                    "{at} declares {} DI modules — one per file, two modules are two folders",
                    found.modules,
                ));
            }
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// The word the adapters in an edge folder are named for, read off the folders
/// above it (`above`, from the repository root down): the module folder — or,
/// when the edge folder sits directly under a framework crate's `src/` or a
/// suite root (the mirror of `src/`), the crate's subject. A product crate's
/// root gives no word: an edge there is refused where it sits.
fn module_word<'a>(above: &'a [Cow<'a, str>]) -> Option<&'a str> {
    match above {
        [area, .., root] if root == "src" && area == PRODUCT => None,
        [.., krate, root] if root == "src" => Some(subject_of(krate)),
        [.., krate, tests, _suite] if tests == "tests" => Some(subject_of(krate)),
        [.., module] => Some(module),
        [] => None,
    }
}

/// Every struct an item list declares, inline modules included and
/// `#[cfg(test)]` ones left out.
fn structs_at_any_depth(items: &[Item]) -> Vec<&syn::ItemStruct> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Struct(s) if !is_cfg_test(&s.attrs) => out.push(s),
            Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                if let Some((_, inner)) = &m.content {
                    out.extend(structs_at_any_depth(inner));
                }
            }
            _ => {}
        }
    }
    out
}

/// **An edge adapter is named for the module it adapts** —
/// `posts/http/controller.rs` is `PostsController`, never `HttpController`,
/// which would name the adapter twice and the thing it adapts never.
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

    for area in ["crates", PRODUCT] {
        for path in rust_files(&root.join(area)) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((_, role)) = ROLES.iter().find(|(f, _)| *f == name) else {
                continue;
            };
            let parts = segments(&path, &root);
            // …/<module>/<edge>/<role>.rs — the edge folder is what marks the
            // file as an adapter rather than a module-root role file.
            let Some(edge_at) = parts.len().checked_sub(2) else {
                continue;
            };
            if !EDGES.contains(&&*parts[edge_at]) {
                continue;
            }
            let Some(module) = module_word(&parts[..edge_at]).map(folded) else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            for declared in structs_at_any_depth(&ast.items) {
                let ident = declared.ident.to_string();
                if !ident.ends_with(role) {
                    continue;
                }
                scanned += 1;
                if !folded(&ident).starts_with(&module) {
                    offenders.push(format!("{ident} in {}", relative(&path, &root)));
                }
            }
        }
    }
    assert!(
        scanned >= 12,
        "read {scanned} edge adapter(s), expected at least 12"
    );
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "an adapter is named for the module it adapts, so a reader finds it from \
         the feature rather than from the transport:\n  {}",
        offenders.join("\n  "),
    );
}

/// **In a product crate, an edge adapter sits in a module folder** — never
/// directly under an app's or a library's `src/`, whether as `src/http/` or as
/// `src/http.rs`. The app name stops at `<App>Module` and a product library is a
/// container of modules, so an adapter there adapts nothing a reader can name.
#[test]
fn an_edge_adapter_in_a_product_crate_sits_in_a_module_folder() {
    let root = repo_root();
    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    for workspace in PRODUCT_WORKSPACES {
        let Ok(crates) = std::fs::read_dir(root.join(PRODUCT).join(workspace)) else {
            continue;
        };
        for krate in crates.flatten() {
            let Ok(entries) = std::fs::read_dir(krate.path().join("src")) else {
                continue;
            };
            scanned += 1;
            for entry in entries.flatten() {
                let path = entry.path();
                let module = if path.is_dir() {
                    path.file_name()
                } else if path.extension().is_some_and(|e| e == "rs") {
                    path.file_stem()
                } else {
                    continue;
                };
                let Some(edge) = module
                    .and_then(|m| m.to_str())
                    .filter(|m| EDGES.contains(m))
                else {
                    continue;
                };
                offenders.push(format!(
                    "{} — move it to `src/<module>/{edge}/`, named for the module it adapts",
                    relative(&path, &root),
                ));
            }
        }
    }
    assert!(
        scanned >= 6,
        "read {scanned} product source root(s), expected at least 6"
    );
    offenders.sort();
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// **No `*_module.rs`, ever.** A DI module is `module.rs` in its own folder.
#[test]
fn no_file_is_named_for_a_module_instead_of_being_one() {
    let root = repo_root();
    let offenders: Vec<String> = ["crates", PRODUCT]
        .iter()
        .flat_map(|area| rust_files(&root.join(area)))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_module.rs"))
        })
        .map(|path| relative(&path, &root))
        .collect();
    assert!(
        offenders.is_empty(),
        "a DI module is `module.rs` in its own folder, never `<name>_module.rs`: {offenders:#?}",
    );
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

/// **No module invents a folder to group things that go together** — the six
/// names the rule itself enumerates. Only folders inside a `src/` tree are
/// module vocabulary: a crate may be called `nest-rs-core`, and a suite may keep
/// what its siblings share in a `common/`.
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
    for area in ["crates", PRODUCT] {
        walk_dirs(&root.join(area), &mut |dir| {
            let parts = segments(dir, &root);
            if parts.iter().any(|part| part == "src")
                && parts.last().is_some_and(|name| INVENTED.contains(&&**name))
            {
                offenders.push(relative(dir, &root));
            }
        });
    }
    assert!(
        offenders.is_empty(),
        "each of these is a bag, and every file in it is already named by a role \
         table — split the module instead: {offenders:#?}",
    );
}

/// **A module may not take a name from the structural vocabulary.** The
/// population is the folders directly under a product `src/`, which is where a
/// module lives; the same words are legal one level down (`http/`, `dtos/`).
/// The edge words are left to the placement rule, whose remedy is a move rather
/// than a rename. The list is the one `nestrs new` refuses a feature name with,
/// derived by the CLI from the `architecture.md` it ships.
#[test]
fn no_module_takes_a_name_from_the_structural_vocabulary() {
    let root = repo_root();
    let reserved = nest_rs_cli::reserved_words();
    assert!(
        reserved.len() > 20,
        "the CLI derived {} reserved word(s), expected more than 20",
        reserved.len(),
    );

    let mut module_roots = vec![root.join(PRODUCT).join("crates/features/src")];
    if let Ok(apps) = std::fs::read_dir(root.join(PRODUCT).join("apps")) {
        module_roots.extend(apps.flatten().map(|a| a.path().join("src")));
    }
    let mut offenders = Vec::new();
    for src in module_roots {
        for entry in std::fs::read_dir(&src).into_iter().flatten().flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if entry.path().is_dir()
                && reserved.contains(name.as_str())
                && !EDGES.contains(&name.as_str())
            {
                offenders.push(relative(&entry.path(), &root));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a module named from the layout's own vocabulary makes every path below \
         it ambiguous — pick the domain word: {offenders:#?}",
    );
}

/// **A file and its folder, read together, spell the type** — run through
/// `nest_rs_cli::lint`, the same code `nestrs lint` runs in a developer's
/// project, so the rule the framework ships is the rule it meets.
#[test]
fn every_file_is_named_for_what_it_declares() {
    let scan = nest_rs_cli::lint::scan(&repo_root());
    assert!(
        scan.checked >= 150,
        "`nestrs lint` checked {} file(s), expected at least 150",
        scan.checked,
    );
    let offenders: Vec<String> = scan.findings.iter().map(ToString::to_string).collect();
    assert!(offenders.is_empty(), "{}", offenders.join("\n\n"));
}

/// A `#[config]` struct as shipped: where it sits and what it declares.
struct Declared {
    krate: PathBuf,
    path: PathBuf,
    folders: Vec<String>,
    ident: String,
    namespace: String,
}

/// Every `#[config]` both workspaces ship, at any depth outside `#[cfg(test)]`.
fn configs() -> Vec<Declared> {
    fn collect(items: &[Item], found: &mut Vec<(String, String)>) {
        for item in items {
            match item {
                Item::Struct(s) if !is_cfg_test(&s.attrs) => {
                    if let Some(namespace) = config_namespace(&s.attrs) {
                        found.push((s.ident.to_string(), namespace));
                    }
                }
                Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                    if let Some((_, inner)) = &m.content {
                        collect(inner, found);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for krate in crate_dirs() {
        let src = krate.join("src");
        for path in rust_files(&src) {
            let Some(ast) = parsed(&path) else { continue };
            let mut found = Vec::new();
            collect(&ast.items, &mut found);
            let parts = segments(&path, &src);
            let folders: Vec<String> = parts[..parts.len() - 1]
                .iter()
                .map(|s| s.to_string())
                .collect();
            for (ident, namespace) in found {
                out.push(Declared {
                    krate: krate.clone(),
                    path: path.clone(),
                    folders: folders.clone(),
                    ident,
                    namespace,
                });
            }
        }
    }
    assert!(
        out.len() >= 15,
        "read {} `#[config]`(s), expected at least 15",
        out.len()
    );
    out
}

/// The pluralised role folders, plus `providers/`: a role folder groups files of
/// one role and is not a level of the name. `events` is absent although it is a
/// role folder too: it is also an edge, and an edge's config owes its segment.
const PLURAL_ROLE_FOLDERS: [&str; 7] = [
    "services",
    "entities",
    "dtos",
    "commands",
    "strategies",
    "pipes",
    "providers",
];

/// The leading segments of a framework crate's namespace: its subject, folded —
/// or, for a member of a family, the family and the member as two levels, read
/// off the `TARGET` the crate declares (`nest_rs::oauth::client` → `oauth`,
/// `client`), so the variable, the span target and the path agree.
fn subject_segments(krate: &str) -> Vec<String> {
    let own = declared_targets()
        .iter()
        .filter(|(_, owner, konst)| *owner == krate && *konst == "TARGET")
        .filter_map(|(target, _, _)| target.strip_prefix("nest_rs::"))
        .find(|tail| tail.contains("::"));
    match own {
        Some(tail) => tail.split("::").map(folded).collect(),
        None => vec![folded(subject_of(krate))],
    }
}

/// **A `#[config]`'s namespace is its stem** — the crate's subject, then every
/// binding folder below `src/`, joined by `__`: `redis/src/worker/config.rs` is
/// `redis__worker`, `social/src/providers/github/config.rs` is `social__github`.
/// A product crate is a container, named only where nothing below it names the
/// file: `features/src/oauth/config.rs` is `oauth`.
#[test]
fn namespace_is_the_stem() {
    let root = repo_root();
    let framework = root.join("crates");
    let mut holes = Vec::new();
    for config in configs() {
        let krate = crate_name(&config.krate);
        let below: Vec<String> = config
            .folders
            .iter()
            .filter(|f| !PLURAL_ROLE_FOLDERS.contains(&f.as_str()))
            .map(|f| folded(f))
            .collect();
        let expected: Vec<String> = if config.krate.starts_with(&framework) {
            subject_segments(krate).into_iter().chain(below).collect()
        } else if below.is_empty() {
            vec![folded(subject_of(krate))]
        } else {
            below
        };
        let declared: Vec<String> = config.namespace.split("__").map(folded).collect();
        if declared != expected {
            holes.push(format!(
                "`{}` is declared by {} — its stem says `{}`",
                config.namespace,
                relative(&config.path, &root),
                expected.join("__"),
            ));
        }
    }
    holes.sort();
    assert!(
        holes.is_empty(),
        "a namespace chosen rather than read off the path — move the config to where \
         its word belongs, or take the word of where it sits:\n  {}",
        holes.join("\n  "),
    );
}

/// **A `#[config]`'s type is named from the stem its namespace is** —
/// `RedisWorkerConfig` for `redis__worker`; a provider's takes the member-first
/// name the role tables give a provider's files (`GithubSocialConfig`).
#[test]
fn a_config_is_named_for_its_stem() {
    let root = repo_root();
    let mut holes = Vec::new();
    for config in configs() {
        let member = config
            .folders
            .iter()
            .position(|f| f == "providers")
            .and_then(|at| config.folders.get(at + 1));
        let mut words: Vec<&str> = config.namespace.split("__").collect();
        if let Some(member) = member
            && let Some(at) = words.iter().position(|word| word == member)
        {
            let member = words.remove(at);
            words.insert(0, member);
        }
        let expected = format!(
            "{}Config",
            words.iter().map(|w| pascal(w)).collect::<String>()
        );
        if folded(&expected) != folded(&config.ident) {
            holes.push(format!(
                "`{}` is declared by {} under `{}` — its stem names `{expected}`",
                config.ident,
                relative(&config.path, &root),
                config.namespace,
            ));
        }
    }
    holes.sort();
    assert!(
        holes.is_empty(),
        "a config whose type and variable name two different things — rename the \
         type to the stem its namespace and its path agree on:\n  {}",
        holes.join("\n  "),
    );
}

/// **No namespace is a near miss of another**, by the rule the loader's
/// unclaimed-variable report applies: that report stays silent on a namespace
/// it does not link only while no two namespaces a deployment sets side by side
/// read as one misspelled for the other.
#[test]
fn no_namespace_is_a_near_miss_of_another() {
    let root = repo_root();
    let declared: Vec<(String, String)> = configs()
        .into_iter()
        .map(|c| (c.namespace, relative(&c.path, &root)))
        .collect();
    let mut near = BTreeSet::new();
    for (i, (a, at)) in declared.iter().enumerate() {
        for (b, bt) in &declared[i + 1..] {
            if a != b && nest_rs_config::unclaimed::is_near_miss(a, b) {
                near.insert(format!("`{a}` ({at}) and `{b}` ({bt})"));
            }
        }
    }
    assert!(
        near.is_empty(),
        "namespaces the unclaimed-variable report would read as one misspelled for \
         the other — a binary linking one would report the other's variables; rename \
         one:\n{}",
        near.into_iter().collect::<Vec<_>>().join("\n"),
    );
}
