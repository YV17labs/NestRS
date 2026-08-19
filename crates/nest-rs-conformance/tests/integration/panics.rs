//! The panics join: every seam that contains a panic, against the one field
//! name it is logged under.
//!
//! A framework seam that catches a panic rather than letting it unwind has to
//! put something structured in the log, and `nest_rs_core::panic` exists because
//! three crates had written the same downcast ladder and **already drifted on
//! the fallback string and on the field name they logged it under**. The ladder
//! and the fallback were then shared; the field name was left in prose, which is
//! the half that drifts without saying so.
//!
//! `nest_rs_core::panic::FIELD` is the declaration. It cannot be *read* at an
//! emit site — a `tracing` field name is a literal token in the macro's grammar
//! — so the only thing that can hold the sites to it is a join, and this is it.
//!
//! Members are derived, never listed: every file under `crates/` and `demo/`
//! that calls `panic_message`. The list was three crate names for one round,
//! which is the shape `testing.md` clause 1 names as the defect — a fourth
//! containment seam is exactly what the framework grows when an edge learns to
//! catch, and a hand-written population cannot see one.

use std::collections::BTreeSet;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, declared_str, flatten, read, relative, repo_root, rust_files,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// Three seams contain a panic today — the scheduler, the event bus and the
/// queue consumer. Below that the scan is reading the wrong tree.
const FLOOR: usize = 3;

#[test]
fn every_contained_panic_is_logged_under_the_declared_field() {
    let root = repo_root();

    // The declaration, read rather than linked: this crate proves things *about*
    // the framework, and depending on it to learn one string would put the whole
    // tree behind the test binary.
    let field = declared_str(&root.join("crates/nest-rs-core/src/panic.rs"), "FIELD")
        .expect("`panic::FIELD` is declared as a string literal");

    let mut seams = 0usize;
    let mut wrong: BTreeSet<String> = BTreeSet::new();
    for dir in crate_dirs() {
        for path in rust_files(&dir.join("src")) {
            let Ok(raw) = read(&path) else {
                continue;
            };
            // Comments carry the *prose about* these two strings — this join's
            // own subject is a field name, and a doc table naming it would
            // enrol a file as a containment seam and close its cell in one
            // stroke. `nest-rs-macro-hygiene`'s scan blanks them for the mirror
            // reason: "Both would report a violation that does not exist."
            let text = without_comments(&raw);
            // The definition is not a seam: `nest_rs_core::panic` declares
            // `panic_message` and names it in its own doc, and it logs nothing.
            // Derived from the declaration rather than from the path, so moving
            // the module does not silently empty the population.
            if !text.contains("panic_message(") || text.contains("pub fn panic_message(") {
                continue;
            }
            seams += 1;
            // **Every call, inside a `tracing` event that names the field** —
            // not "the file mentions both somewhere". Two whole-file
            // `contains` let a file with two containment sites pass on one of
            // them, and `FIELD` is the literal `panic`, so a `let panic = …`
            // binding satisfied the second check as readily as a rendered
            // field. Both are the substring reading `testing.md` clause 3
            // forbids: the cell has to fail when the *behaviour* goes.
            if logged_calls(&raw, &field) != call_sites(&raw) {
                wrong.insert(relative(&path, &root));
            }
        }
    }

    baseline::floor(seams, FLOOR, "containment seam(s)");
    assert!(
        wrong.is_empty(),
        "{} site(s) render a caught panic under a field other than `{field}`, so one \
         query no longer reaches them all:\n  {}",
        wrong.len(),
        wrong.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}

/// How many `panic_message(` calls this file makes, comments excluded.
fn call_sites(raw: &str) -> usize {
    let mut count = 0usize;
    let Ok(tokens) = without_comments(raw).parse::<TokenStream>() else {
        return 0;
    };
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);
    for pair in flat.windows(2) {
        if let [TokenTree::Ident(ident), TokenTree::Group(args)] = pair
            && ident == "panic_message"
            && args.delimiter() == Delimiter::Parenthesis
        {
            count += 1;
        }
    }
    count
}

/// How many of them reach a `tracing` event that renders `field`.
///
/// The event, because that is the whole claim: a caught panic reaches one query
/// only if the value goes out *as that field*. A macro invocation is where a
/// `tracing` field name exists at all — it is a literal token in the macro's
/// grammar, which is why `panic::FIELD` cannot be read at an emit site and why
/// this join exists.
///
/// **Through a local binding as well as inline**, and that is not a loosening:
/// `nest-rs-redis`'s consumer writes `let detail = panic_message(payload…);`
/// and renders `panic = %detail` two lines down, which is the same behaviour
/// spelled over two statements. Requiring the call to sit lexically inside the
/// event reported it as a hole — a check that fails on a correct site is worth
/// less than none, because the fix a reader reaches for is to inline code that
/// was clearer as it was.
fn logged_calls(raw: &str, field: &str) -> usize {
    let Ok(tokens) = without_comments(raw).parse::<TokenStream>() else {
        return 0;
    };
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);

    // `let <name> = panic_message(…)` — the names a field render may carry.
    let mut bound: BTreeSet<String> = BTreeSet::new();
    for window in flat.windows(5) {
        if let [
            TokenTree::Ident(let_kw),
            TokenTree::Ident(name),
            TokenTree::Punct(eq),
            TokenTree::Ident(call),
            TokenTree::Group(_),
        ] = window
            && let_kw == "let"
            && eq.as_char() == '='
            && call == "panic_message"
        {
            bound.insert(name.to_string());
        }
    }

    let mut count = 0usize;
    for window in flat.windows(2) {
        // `tracing::error!( … )` flattens to `… ! Group`, and the group is the
        // event's whole argument list.
        let [TokenTree::Punct(bang), TokenTree::Group(body)] = window else {
            continue;
        };
        if bang.as_char() != '!' || body.delimiter() != Delimiter::Parenthesis {
            continue;
        }
        let mut inner = Vec::new();
        flatten(body.stream(), &mut inner);
        // Every `field = <expr>` in this event whose value carries the panic —
        // the call itself, or a name bound from it.
        for (at, pair) in inner.windows(2).enumerate() {
            let carries = matches!(pair, [TokenTree::Ident(ident), TokenTree::Punct(eq)]
                if ident == field && eq.as_char() == '=');
            if !carries {
                continue;
            }
            // **`let panic = …` is not a field render**, and `FIELD` being the
            // literal `panic` is what makes that worth a line: a binding is
            // `ident =` too, so without this the statement that *withholds* the
            // value from the event satisfied the check that it reaches it.
            if matches!(inner.get(at.wrapping_sub(1)), Some(TokenTree::Ident(kw)) if kw == "let") {
                continue;
            }
            let value = inner[at + 2..]
                .iter()
                .take_while(|tree| !matches!(tree, TokenTree::Punct(p) if p.as_char() == ','));
            if value.into_iter().any(|tree| {
                matches!(tree, TokenTree::Ident(ident)
                    if ident == "panic_message" || bound.contains(&ident.to_string()))
            }) {
                count += 1;
            }
        }
    }
    count
}

/// The source with `//`-comments blanked, so prose *about* a field name cannot
/// stand in for a line that logs under it.
///
/// Line comments only, and that is enough here: `crates/*/src` carries no block
/// comment outside a `///` line, which this drops whole.
fn without_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
