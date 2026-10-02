//! The upgrade join: every struct the upgrade page tells a 6.x reader to write
//! out whole is answered with what makes the literal compile again.
//!
//! Step 5 of `upgrading.mdx` is a table of compile errors, and a row naming a
//! struct literal — ``a `CronJobMeta { … }` registered by hand`` — promises that
//! following it compiles. That promise has one of two shapes, read off the
//! struct itself:
//!
//! - **The struct has a `Default`**: the row ends the literal in
//!   `..Default::default()`, which keeps compiling whatever fields arrive.
//! - **It has none**: the row names every field 7.0 added, as `field:`. The row
//!   for `CronJobMeta` named `origin` alone while 7.0 had also added `replicas`,
//!   so a reader following it met `E0063: missing field` — the compiler found
//!   the row's gap, which is exactly what the row exists to spare.
//!
//! The 6.1 fields are **stated**, per struct, in [`SIX_ONE_FIELDS`]: the release
//! a 6.x reader upgrades from is fixed, and history is not a tree this suite can
//! read. A struct without a `Default` and without a stated 6.1 shape fails here,
//! so a row added for one cannot skip the question.
//!
//! **What it reads by its spelling** — declared to the `blinds` join in
//! [`followed`] — is each struct of [`SIX_ONE_FIELDS`] and the `Default` it may
//! implement; the `blinds` join refuses each renamed, aliased or written by a
//! `macro_rules!`.

use std::collections::BTreeSet;

use crate::Followed;
use nest_rs_conformance::sources::{each_source, read, repo_root};

/// The page.
const PAGE: &str = "docs/src/content/docs/upgrading.mdx";

/// The public fields each struct without a `Default` had in 6.1, the release a
/// 6.x reader upgrades from — `crates/nest-rs-schedule/src/inventory.rs` at
/// `v6.1.0`.
const SIX_ONE_FIELDS: [(&str, &[&str]); 1] = [(
    "CronJobMeta",
    &["provider", "method", "trigger", "run", "transaction"],
)];

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    let mut followed = vec![Followed::implemented("Default")];
    followed.extend(
        SIX_ONE_FIELDS
            .iter()
            .map(|(name, _)| Followed::type_(*name)),
    );
    followed
}

/// A struct the page names as a literal to write out: `` `Name { … }` ``.
fn literals(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|code| code.strip_suffix(" { … }"))
        .map(str::to_owned)
        .collect()
}

/// What the tree says of the struct `name`: its public fields, and whether it
/// has a `Default` — derived, or implemented by hand anywhere.
fn shape(name: &str) -> Option<(BTreeSet<String>, bool)> {
    let root = repo_root();
    let mut fields = None;
    let mut default = false;
    each_source(&root, |file, ast| {
        if file.contains("/tests/") || file.starts_with("demo/") {
            return;
        }
        for item in &ast.items {
            match item {
                syn::Item::Struct(item) if item.ident == name => {
                    default |= item.attrs.iter().any(|attr| {
                        attr.path().is_ident("derive")
                            && attr
                                .parse_args_with(
                                    syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                                )
                                .is_ok_and(|paths| paths.iter().any(|path| path.is_ident("Default")))
                    });
                    fields = Some(
                        item.fields
                            .iter()
                            .filter(|field| matches!(field.vis, syn::Visibility::Public(_)))
                            .filter_map(|field| field.ident.as_ref().map(ToString::to_string))
                            .collect::<BTreeSet<_>>(),
                    );
                }
                syn::Item::Impl(item) => {
                    let names_default = item
                        .trait_
                        .as_ref()
                        .is_some_and(|(path, _)| path.is_ident("Default"));
                    let for_name =
                        matches!(&*item.self_ty, syn::Type::Path(ty) if ty.path.is_ident(name));
                    default |= names_default && for_name;
                }
                _ => {}
            }
        }
    });
    fields.map(|fields| (fields, default))
}

#[test]
fn a_struct_written_out_whole_is_given_what_compiles_it_again() {
    let page = read(&repo_root().join(PAGE)).expect("the upgrade page reads");
    let rows: Vec<(String, String)> = page
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim().strip_prefix('|')?.split(" | ").collect();
            let [from, to] = cells.as_slice() else {
                return None;
            };
            Some((
                from.trim().to_owned(),
                to.trim().trim_end_matches('|').to_owned(),
            ))
        })
        .collect();
    let mut answered = 0;
    let mut findings = Vec::new();
    for (from, to) in &rows {
        for name in literals(from) {
            answered += 1;
            let Some((fields, default)) = shape(&name) else {
                findings.push(format!(
                    "`{name}` is named on the page and declared nowhere"
                ));
                continue;
            };
            if default {
                if !to.contains("..Default::default()") {
                    findings.push(format!(
                        "`{name}` has a `Default`, and its row does not end the literal in \
                         `..Default::default()`: {to}"
                    ));
                }
                continue;
            }
            let Some((_, before)) = SIX_ONE_FIELDS.iter().find(|(known, _)| *known == name) else {
                findings.push(format!(
                    "`{name}` has no `Default`: state its 6.1 fields in SIX_ONE_FIELDS, so its \
                     row is held to the fields 7.0 added"
                ));
                continue;
            };
            for gained in fields
                .iter()
                .filter(|field| !before.contains(&field.as_str()))
            {
                if !to.contains(&format!("`{gained}:")) {
                    findings.push(format!(
                        "`{name}` gained `{gained}` in 7.0, and its row does not name it — a \
                         reader following it meets E0063: {to}"
                    ));
                }
            }
        }
    }
    assert!(
        answered >= 3,
        "the page's struct rows were not found: {rows:#?}"
    );
    assert!(findings.is_empty(), "{findings:#?}");
}
