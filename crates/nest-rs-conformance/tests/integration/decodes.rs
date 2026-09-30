//! The decodes join: every `#[config]`'s `from_env`, against the rule that a
//! structured value is decoded by `ConfigService::json` and never by the config
//! itself.
//!
//! `CLAUDE.md`, *A decode failure is said without its value*: a `#[config]` value
//! is a payload too, and a structured one is where a deployment writes client
//! secrets. serde's own sentence quotes the value it refused, so a `from_env`
//! that calls `serde_json::from_str` and hands the error to `Setting::refuse` —
//! the demo's `IssuerConfig` did, for its OAuth client list — prints that list's
//! secrets into the boot error. `ConfigService::json` decodes once, in
//! `nest-rs-config`, and refuses through `nest_rs_core::DecodeError`.
//!
//! Members are derived, never listed: every `impl Config for …` under `crates/`
//! and `demo/`, its whole body read as tokens, so a helper closure or a
//! `use serde_json::from_str` inside it is seen as readily as a direct call. A
//! helper *function* outside the impl is the known looseness.

use std::collections::BTreeSet;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{each_source, repo_root};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

/// `impl Config for` blocks the walk must find. The framework and the demo hold
/// some two dozen; below this the scan is reading the wrong tree.
const FLOOR: usize = 15;

#[test]
fn no_config_decodes_a_structured_value_itself() {
    let root = repo_root();
    let mut impls = 0usize;
    let mut wrong: BTreeSet<String> = BTreeSet::new();
    each_source(&root, |rel, ast| {
        let mut found = ConfigImpls::default();
        found.visit_file(ast);
        impls += found.impls;
        for ty in found.decoding {
            wrong.insert(format!("{rel}: {ty}"));
        }
    });

    baseline::floor(impls, FLOOR, "`impl Config for` block(s)");
    assert!(
        wrong.is_empty(),
        "{} `#[config]`(s) decode a structured value with `serde_json` in their own \
         `from_env`, so a value that does not decode is refused in serde's words — which \
         quote it. Read it with `env.json::<T>(\"KEY\")?` instead:\n  {}",
        wrong.len(),
        wrong.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

/// The `impl Config for` blocks of one file, and the types whose block names
/// `serde_json`.
#[derive(Default)]
struct ConfigImpls {
    impls: usize,
    decoding: Vec<String>,
}

impl<'ast> Visit<'ast> for ConfigImpls {
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let is_config = item
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|last| last.ident == "Config");
        if is_config {
            self.impls += 1;
            if names_serde_json(item.to_token_stream()) {
                self.decoding
                    .push(item.self_ty.to_token_stream().to_string());
            }
        }
        syn::visit::visit_item_impl(self, item);
    }
}

fn names_serde_json(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => ident == "serde_json",
        TokenTree::Group(group) => names_serde_json(group.stream()),
        _ => false,
    })
}

/// The join is proved on the shape it exists to catch: the demo's
/// `IssuerConfig` as it stood, and the same body reading through `env.json`.
#[test]
fn the_join_sees_a_config_decoding_its_own_value() {
    let caught: syn::File = syn::parse_quote! {
        impl Config for IssuerConfig {
            fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                let clients = match env.setting("CLIENTS")? {
                    Some(raw) => serde_json::from_str(&raw.value).map_err(|e| raw.refuse(e))?,
                    None => base.clients,
                };
                Ok(Self { clients })
            }
        }
    };
    let passed: syn::File = syn::parse_quote! {
        impl nest_rs::config::Config for IssuerConfig {
            fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                Ok(Self { clients: env.json("CLIENTS")?.unwrap_or(base.clients) })
            }
        }
    };
    let mut found = ConfigImpls::default();
    found.visit_file(&caught);
    assert_eq!(found.impls, 1);
    assert_eq!(found.decoding, ["IssuerConfig"]);
    let mut found = ConfigImpls::default();
    found.visit_file(&passed);
    assert_eq!(found.impls, 1);
    assert!(found.decoding.is_empty());
}
