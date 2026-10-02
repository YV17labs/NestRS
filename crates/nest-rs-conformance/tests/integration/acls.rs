//! The ACL join: the Redis ACL rule each binding's page prescribes names every
//! command the binding sends, and nothing else.
//!
//! **Redis checks a script's own commands against the caller's ACL**, not only
//! the `EVALSHA` that carries them, so a rule naming the scripts and not what
//! they call refuses every call. That is where the schedule page and the rate
//! limiter's stood: each told an operator to allow `EVALSHA` and `SCRIPT LOAD`,
//! and a user allowed exactly that skipped every occurrence and failed every hit.
//!
//! Each binding's page holds one ```` ```text title="Redis ACL" ```` block, and
//! `nest-rs-redis`'s e2e creates a user from that line, verbatim, and runs the
//! binding through it — which proves the paths a test takes. This join proves
//! the rest without a Redis, both ways: every command the framework's own
//! sources send for a binding is in its rule, and every command its rule allows
//! is one something sends — least privilege is a rule that grants nothing
//! unused, and a command dropped from the code would otherwise stay granted.
//!
//! **What a binding sends** is read from its files under
//! `crates/nest-rs-redis/src`, `#[cfg(test)]` items left out: the command of
//! every `cmd("…")`, every `redis.call('…')` / `redis.pcall('…')` in a Lua
//! script, and `EVALSHA` with `SCRIPT LOAD` wherever a `Script` is built — the
//! two a script reaches Redis through. The connection's file is every binding's,
//! and so is the `SELECT` the client's handshake sends for the database a URL
//! names. apalis-redis's commands are a dependency's, so they are stated —
//! [`APALIS_COMMANDS`], read off the scripts the queue runs — and pinned to the
//! release, as the keys join pins apalis's layout.
//!
//! **What it refuses** is a way to Redis whose command it cannot read: a `cmd`
//! whose command is not one literal, and the command traits and packed sends in
//! [`UNREAD_WAYS`]. What it cannot see, and a review owes: a typed helper called
//! on a `Pipeline` (`.lrem(…)`), which reaches Redis through `pipe` without
//! naming its command as a literal — none is written in a binding today.
//!
//! **What it reads by its spelling** — declared to the `blinds` join in
//! [`followed`] — is `cmd`, `Script` and the ways in [`UNREAD_WAYS`]; the
//! `blinds` join refuses each renamed, aliased or written by a `macro_rules!`.
//!
//! **Every file sending a command belongs to a binding or is named here.** The
//! 6.x check is the one exemption: it reads the queue's bare name at the root of
//! the keyspace, which every rule refuses on purpose, and says at `warn` that it
//! could not look.

use crate::Followed;
use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::sources::{files_with_extension, is_cfg_test, read, relative, repo_root};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    let mut followed = vec![Followed::call("cmd"), Followed::type_("Script")];
    followed.extend(UNREAD_WAYS.iter().map(|way| {
        if way.starts_with(char::is_uppercase) {
            Followed::type_(*way)
        } else {
            Followed::call(*way)
        }
    }));
    followed
}

/// Where every Redis binding lives.
const REDIS_SRC: &str = "crates/nest-rs-redis/src";

/// Where the pages are.
const CORPUS: &str = "docs/src/content/docs";

/// The fence a page's rule sits under.
const FENCE: &str = "```text title=\"Redis ACL\"";

/// A binding: its files under [`REDIS_SRC`] (a folder ends with `/`), the page
/// prescribing its rule, the keys that rule confines it to, and whether it runs
/// apalis's scripts.
struct Binding {
    name: &'static str,
    files: &'static [&'static str],
    page: &'static str,
    keys: &'static str,
    apalis: bool,
}

/// Every Redis binding, each with the page an operator reads its rule on.
const BINDINGS: [Binding; 3] = [
    Binding {
        name: "queue",
        files: &[
            "queue/",
            "worker/",
            "backend.rs",
            "layout.rs",
            "promotion.rs",
        ],
        page: "queue/delivery.mdx",
        keys: "~nestrs:queue:*",
        apalis: true,
    },
    Binding {
        name: "rate limiter",
        files: &["throttler/"],
        page: "rate-limiting/index.mdx",
        keys: "~nestrs:throttler:*",
        apalis: false,
    },
    Binding {
        name: "schedule",
        files: &["schedule/"],
        page: "schedule/index.mdx",
        keys: "~nestrs:schedule:*",
        apalis: false,
    },
];

/// The connection every binding runs over: what it sends, every rule allows. It
/// is also the one file that implements the ways to Redis in [`UNREAD_WAYS`],
/// forwarding what its callers send rather than sending a command of its own,
/// so it is not held to them.
const CONNECTION: &str = "connection.rs";

/// The file whose commands no rule allows, on purpose — the 6.x check, reading
/// outside the framework's namespace.
const UNRULED: &str = "legacy_layout.rs";

/// What the client's handshake sends for every binding, though no source spells
/// it: the `SELECT` of the database a URL names.
const HANDSHAKE: [&str; 1] = ["SELECT"];

/// How a script reaches Redis: by its digest, and loaded the first time Redis
/// has not cached it.
const SCRIPT_COMMANDS: [&str; 2] = ["EVALSHA", "SCRIPT|LOAD"];

/// The commands of the apalis-redis scripts the queue runs — `push_job`,
/// `schedule_job`, `get_jobs`, `register_consumer`, `enqueue_scheduled_jobs`,
/// `reenqueue_orphaned_jobs`, `done_job`, `kill_job` and `retry_job` under
/// `apalis-redis-0.7.4/lua/` — and of the calls it sends itself from an
/// acknowledgement and a reschedule (`HMGET`, `HSET`, `SREM`, `ZADD`).
///
/// **Stated rather than derived, and it has to be:** the scripts are a
/// dependency's, which nothing here can read. Pinned to [`APALIS_REDIS_PIN`] by
/// [`the_apalis_commands_read_here_are_the_pinned_releases`], so the list is
/// re-read with a move of the pin rather than trusted past it.
const APALIS_COMMANDS: [&str; 19] = [
    "DEL",
    "HDEL",
    "HGET",
    "HMGET",
    "HMSET",
    "HSET",
    "HSETNX",
    "LPUSH",
    "LRANGE",
    "LTRIM",
    "RPUSH",
    "SADD",
    "SPOP",
    "SREM",
    "ZADD",
    "ZRANGEBYSCORE",
    "ZREM",
    "ZREMRANGEBYRANK",
    "ZSCORE",
];

/// The apalis-redis requirement [`APALIS_COMMANDS`] was read off.
const APALIS_REDIS_PIN: &str = "0.7";

/// Ways to Redis that send a command this join cannot read as a literal.
const UNREAD_WAYS: [&str; 8] = [
    "Commands",
    "AsyncCommands",
    "TypedCommands",
    "AsyncTypedCommands",
    "req_packed_command",
    "req_packed_commands",
    "send_packed_command",
    "send_packed_commands",
];

/// What one file sends, and the ways it reaches Redis that this join cannot read.
#[derive(Default)]
struct Sent {
    commands: BTreeSet<String>,
    unread: Vec<String>,
}

/// Read what `path` sends, its `#[cfg(test)]` items left out.
fn sent(path: &Path) -> Sent {
    let text = read(path).expect("a binding's file reads");
    let file = syn::parse_file(&text).expect("a binding's file parses");
    let mut out = Sent::default();
    for item in &file.items {
        let attrs = match item {
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            _ => &Vec::new(),
        };
        if !is_cfg_test(attrs) {
            read_tokens(item.to_token_stream(), &mut out);
        }
    }
    out
}

/// Read `tokens` at every depth. An attribute is skipped whole: a doc comment
/// naming a call is prose, not a script.
fn read_tokens(tokens: TokenStream, out: &mut Sent) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut attribute = false;
    for (at, token) in tokens.iter().enumerate() {
        if std::mem::take(&mut attribute) {
            continue;
        }
        match token {
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                attribute = matches!(
                    tokens.get(at + 1),
                    Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket
                );
            }
            TokenTree::Group(group) => read_tokens(group.stream(), out),
            TokenTree::Literal(literal) => {
                if let Ok(lit) = syn::parse2::<syn::LitStr>(literal.to_token_stream()) {
                    out.commands.extend(lua_calls(&lit.value()));
                }
            }
            TokenTree::Ident(ident) if ident == "cmd" => {
                if let Some(TokenTree::Group(args)) = tokens.get(at + 1)
                    && args.delimiter() == Delimiter::Parenthesis
                {
                    match syn::parse2::<syn::LitStr>(args.stream()) {
                        Ok(command) => {
                            out.commands.insert(command.value().to_ascii_uppercase());
                        }
                        Err(_) => out.unread.push(format!("cmd({})", args.stream())),
                    }
                }
            }
            TokenTree::Ident(ident) if ident == "Script" => {
                out.commands
                    .extend(SCRIPT_COMMANDS.iter().map(|command| (*command).to_owned()));
            }
            TokenTree::Ident(ident) if UNREAD_WAYS.contains(&ident.to_string().as_str()) => {
                out.unread.push(ident.to_string());
            }
            _ => {}
        }
    }
}

/// The commands a Lua script calls, upper-cased: `redis.call('INCR', …)`.
fn lua_calls(script: &str) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    for opener in ["redis.call(", "redis.pcall("] {
        for (at, _) in script.match_indices(opener) {
            let rest = script[at + opener.len()..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
                continue;
            };
            let name: String = rest[1..].chars().take_while(|c| *c != quote).collect();
            calls.insert(name.to_ascii_uppercase());
        }
    }
    calls
}

/// The page's rule: the line under its one [`FENCE`].
fn rule(root: &Path, page: &str) -> String {
    let text = read(&root.join(CORPUS).join(page)).expect("a binding's page reads");
    let rules: Vec<&str> = text
        .lines()
        .zip(text.lines().skip(1))
        .filter(|(fence, _)| fence.trim() == FENCE)
        .map(|(_, rule)| rule.trim())
        .collect();
    assert_eq!(
        rules.len(),
        1,
        "{page} prescribes one Redis ACL rule: {rules:?}"
    );
    rules[0].to_owned()
}

/// Whether `file` — relative to [`REDIS_SRC`] — is one of `binding`'s.
fn owns(binding: &Binding, file: &str) -> bool {
    binding.files.iter().any(|owned| {
        if owned.ends_with('/') {
            file.starts_with(owned)
        } else {
            file == *owned
        }
    })
}

/// Every source file of the Redis crate, relative to [`REDIS_SRC`], with what
/// it sends.
fn sources(root: &Path) -> Vec<(String, Sent)> {
    let src = root.join(REDIS_SRC);
    let mut files: Vec<(String, Sent)> = files_with_extension(&src, "rs")
        .into_iter()
        .map(|path| (relative(&path, &src), sent(&path)))
        .collect();
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    files
}

#[test]
fn each_bindings_rule_allows_what_it_sends_and_nothing_else() {
    let root = repo_root();
    let files = sources(&root);
    assert!(
        files.len() >= 10,
        "the Redis crate's sources were not found under {REDIS_SRC}"
    );
    let connection: BTreeSet<String> = files
        .iter()
        .filter(|(file, _)| file == CONNECTION)
        .flat_map(|(_, sent)| sent.commands.iter().cloned())
        .chain(HANDSHAKE.iter().map(|command| (*command).to_owned()))
        .collect();
    assert!(
        connection.contains("PING"),
        "the connection's proof is read: {connection:?}"
    );

    let mut findings = Vec::new();
    for (file, sent) in &files {
        for way in sent.unread.iter().filter(|_| file != CONNECTION) {
            findings.push(format!(
                "{REDIS_SRC}/{file}: `{way}` reaches Redis with a command this join cannot \
                 read — send it through `cmd(\"…\")` or a script"
            ));
        }
        let owned = BINDINGS.iter().any(|binding| owns(binding, file));
        if !sent.commands.is_empty() && !owned && file != CONNECTION && file != UNRULED {
            findings.push(format!(
                "{REDIS_SRC}/{file} sends {:?} and belongs to no binding whose page prescribes \
                 a rule",
                sent.commands
            ));
        }
    }
    for binding in &BINDINGS {
        let rule = rule(&root, binding.page);
        let tokens: Vec<&str> = rule.split_whitespace().collect();
        let allowed: BTreeSet<String> = tokens
            .iter()
            .filter_map(|token| token.strip_prefix('+'))
            .map(str::to_ascii_uppercase)
            .collect();
        let keys: Vec<&&str> = tokens
            .iter()
            .filter(|token| token.starts_with('~'))
            .collect();
        if keys != [&binding.keys] {
            findings.push(format!(
                "{}'s rule confines it to {keys:?}, not to `{}` alone",
                binding.page, binding.keys
            ));
        }
        let mut sends: BTreeSet<String> = files
            .iter()
            .filter(|(file, _)| owns(binding, file))
            .flat_map(|(_, sent)| sent.commands.iter().cloned())
            .chain(connection.iter().cloned())
            .collect();
        if binding.apalis {
            sends.extend(
                APALIS_COMMANDS
                    .iter()
                    .chain(&SCRIPT_COMMANDS)
                    .map(|command| (*command).to_owned()),
            );
        }
        let refused: Vec<_> = sends.difference(&allowed).collect();
        if !refused.is_empty() {
            findings.push(format!(
                "the {} sends {refused:?}, which the rule {} prescribes refuses",
                binding.name, binding.page
            ));
        }
        let unused: Vec<_> = allowed.difference(&sends).collect();
        if !unused.is_empty() {
            findings.push(format!(
                "the rule {} prescribes allows {unused:?}, which the {} never sends",
                binding.page, binding.name
            ));
        }
    }
    assert!(findings.is_empty(), "{findings:#?}");
}

/// A script's own commands are what the rule has to allow; the reader finds
/// them however the call is quoted or spaced, and never mistakes another word.
#[test]
fn a_scripts_calls_are_read_whatever_their_quoting() {
    let calls = lua_calls(
        "local n = redis.call('INCR', KEYS[1])\n\
         if redis.pcall( \"pttl\", KEYS[1]) < 0 then\n\
           redis.call(\"PEXPIRE\", KEYS[1], ARGV[1])\n\
         end\n\
         -- redis.callx('NOT', 1) and call('ALSO_NOT')",
    );
    assert_eq!(
        calls,
        BTreeSet::from(["INCR".to_owned(), "PEXPIRE".to_owned(), "PTTL".to_owned()])
    );
}

/// [`APALIS_COMMANDS`] is apalis-redis 0.7's, so the requirement it was read off
/// is asserted where the list is used: a move of the pin fails here, and the
/// list is re-read with it rather than trusted past it.
#[test]
fn the_apalis_commands_read_here_are_the_pinned_releases() {
    let manifest = read(&repo_root().join("Cargo.toml")).expect("the root manifest reads");
    let parsed: toml_edit::DocumentMut = manifest.parse().expect("the root manifest parses");
    let requirement = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("apalis-redis"))
        .and_then(|dependency| {
            dependency.as_str().or_else(|| {
                dependency
                    .get("version")
                    .and_then(|version| version.as_str())
            })
        });
    assert_eq!(
        requirement,
        Some(APALIS_REDIS_PIN),
        "apalis-redis moved off the release APALIS_COMMANDS was read from — re-read the \
         commands of the scripts the queue runs (`lua/*.lua`) and of the calls it sends \
         itself into the list, then move APALIS_REDIS_PIN",
    );
}
