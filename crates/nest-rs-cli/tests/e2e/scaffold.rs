//! Covers `src/templates/` and `src/commands/generate/` as the tutorial drives
//! them: its pages replayed from the workspace root as a reader follows them,
//! and the suite they end on run against the live Postgres.

use std::path::{Path, PathBuf};

use crate::generated::{cargo, cargo_check, nestrs, patch_to_working_tree, repo};

/// The tutorial pages that build `blog`, in reading order: the checkpoint and
/// the pages after `testing` run or read the reference `api`. Each `nestrs new`
/// or `nestrs g` line of a `bash` block runs from the workspace root; each
/// `rust` block titled with a bare workspace path is that whole file — the
/// reader's when the block marks the lines they change (`{…}`) or the file is
/// new, and otherwise what a generator wrote there. An excerpt takes a title
/// with words in it.
const PAGES: [&str; 6] = [
    "scaffold",
    "entity",
    "http",
    "database",
    "validation",
    "testing",
];

/// What a page asks of the reader, or shows them, in its order.
enum Step {
    /// A `nestrs` command, its arguments after the binary's name.
    Run(Vec<String>),
    /// A file, by its workspace path, and its whole contents; `edited` when
    /// the block marks the lines the reader changes.
    File {
        path: String,
        contents: String,
        edited: bool,
    },
}

/// The steps of `docs/src/content/docs/tutorial/<page>.mdx`.
fn steps(page: &str) -> Vec<Step> {
    let path = repo()
        .join("docs/src/content/docs/tutorial")
        .join(format!("{page}.mdx"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the page {} reads: {e}", path.display()));
    let mut steps = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(fence) = trimmed.strip_prefix("```") else {
            continue;
        };
        let indent = line.len() - trimmed.len();
        let mut body = Vec::new();
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                break;
            }
            body.push(inner.get(indent..).unwrap_or(inner.trim_start()));
        }
        let lang = fence.split_whitespace().next().unwrap_or_default();
        match lang {
            "bash" => steps.extend(body.iter().filter_map(|line| {
                let command = line.trim().trim_start_matches("$ ");
                (command.starts_with("nestrs new ") || command.starts_with("nestrs g ")).then(
                    || {
                        Step::Run(
                            command
                                .split_whitespace()
                                .skip(1)
                                .map(str::to_owned)
                                .collect(),
                        )
                    },
                )
            })),
            "rust" => {
                let titled = fence
                    .split_once("title=\"")
                    .and_then(|(_, rest)| rest.split_once('"'));
                if let Some((path, marks)) = titled.filter(|(title, _)| !title.contains(' ')) {
                    steps.push(Step::File {
                        path: path.to_owned(),
                        contents: format!("{}\n", body.join("\n")),
                        edited: marks.contains('{'),
                    });
                }
            }
            _ => {}
        }
    }
    steps
}

/// Where a page's path lands: a migration's file carries the day it was
/// generated, so the page's date is read as the generated file's.
fn resolve(workspace: &Path, path: &str) -> PathBuf {
    let Some(name) = path
        .strip_prefix("crates/migrations/src/m")
        .and_then(|rest| rest.splitn(3, '_').nth(2))
    else {
        return workspace.join(path);
    };
    let dir = workspace.join("crates/migrations/src");
    let generated: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the migrations crate")
        .map(|entry| entry.expect("an entry").path())
        .filter(|file| {
            file.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('m') && n.ends_with(&format!("_{name}")))
        })
        .collect();
    assert_eq!(
        generated.len(),
        1,
        "the page's `{path}` is the migration `nestrs g migration` wrote, once: {generated:?}"
    );
    generated.into_iter().next().expect("one")
}

/// Hold `shown`, a page's block for a file the reader does not edit, to
/// `on_disk`, the file a generator wrote: a difference names its first line.
fn assert_generated(page: &str, path: &str, on_disk: &str, shown: &str) {
    if on_disk == shown {
        return;
    }
    let (written, paged): (Vec<&str>, Vec<&str>) =
        (on_disk.lines().collect(), shown.lines().collect());
    let line = written
        .iter()
        .zip(&paged)
        .position(|(written, paged)| written != paged)
        .unwrap_or(written.len().min(paged.len()));
    panic!(
        "{page}'s `{path}` shows what the generator wrote there, and differs at line {}:\n  \
         written: {:?}\n  shown:   {:?}\nthe file as written:\n{on_disk}",
        line + 1,
        written.get(line),
        paged.get(line),
    );
}

/// `/tutorial/database/`'s `curl`s, in process: the token route the tutorial's
/// root serves, and the guarded routes its token opens. The test's own, beside
/// the page's: a root that stops serving the route fails it.
const TOKEN_ROUTE_TEST: &str = r#"use nest_rs::http::StatusCode;
use serde_json::json;

use crate::boot;

#[tokio::test]
async fn the_token_route_the_tutorial_curls_opens_the_guarded_routes() {
    let app = boot().await;

    app.http()
        .get("/posts")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let minted = app
        .http()
        .post("/auth/dev-token")
        .body_json(&json!({}))
        .send()
        .await;
    minted.assert_status_is_ok();
    let minted = minted.json().await;
    let bearer = format!(
        "Bearer {}",
        minted.value().object().get("access_token").string()
    );

    app.http()
        .post("/posts")
        .header("authorization", &bearer)
        .body_json(&json!({ "title": "Hello", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED);
    let listed = app
        .http()
        .get("/posts")
        .header("authorization", &bearer)
        .send()
        .await;
    listed.assert_status_is_ok();
    assert_eq!(listed.json().await.value().array().len(), 1);
}
"#;

/// The tutorial, as written: `nestrs new hello` as Getting started leaves the
/// reader, then every page's commands from the workspace root, every file it
/// has them edit, and every file it shows as generated held to the generator's,
/// then the suite the testing page ends on — the generated round trip, its two
/// failure modes, and the token route `blog` serves — run against the live
/// Postgres. `blog`'s root is the generators' alone, so one that stops wiring
/// the app `--app` names, a template drifting from the page that shows it, or
/// a page whose `module.rs` drops an import, fails here.
#[test]
fn the_tutorial_reaches_a_protected_endpoint_as_written() {
    let dir = tempfile::tempdir().expect("a temp dir");
    nestrs(dir.path(), &["new", "hello"], &[]);
    let workspace = dir.path().join("hello");

    let mut ran = Vec::new();
    for page in PAGES {
        for step in steps(page) {
            match step {
                Step::Run(args) => {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    nestrs(&workspace, &args, &[]);
                    ran.push(args.join(" "));
                }
                Step::File {
                    path,
                    contents,
                    edited,
                } => {
                    let file = resolve(&workspace, &path);
                    if edited || !file.exists() {
                        std::fs::write(&file, contents)
                            .unwrap_or_else(|e| panic!("{page}'s `{path}` writes: {e}"));
                    } else {
                        let on_disk = std::fs::read_to_string(&file)
                            .unwrap_or_else(|e| panic!("{page}'s `{path}` reads: {e}"));
                        assert_generated(page, &path, &on_disk, &contents);
                    }
                }
            }
        }
    }
    for expected in [
        "new blog",
        "g resource posts --app blog",
        "g migration create_posts",
    ] {
        assert!(
            ran.iter().any(|command| command == expected),
            "the pages run `nestrs {expected}`: {ran:?}"
        );
    }

    let suite = workspace.join("apps/blog/tests/e2e");
    std::fs::write(suite.join("tutorial.rs"), TOKEN_ROUTE_TEST).expect("the suite is writable");
    let main = std::fs::read_to_string(suite.join("main.rs")).expect("the suite's root");
    std::fs::write(suite.join("main.rs"), format!("{main}\nmod tutorial;\n"))
        .expect("the suite's root is writable");

    patch_to_working_tree(&workspace);
    if let Err(stderr) = cargo_check(&workspace) {
        panic!("the tutorial's workspace does not compile:\n{stderr}");
    }
    let run = cargo(&workspace, &["test", "-p", "blog", "--test", "e2e"], &[])
        .unwrap_or_else(|err| panic!("{err}"));
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        run.status.success(),
        "the tutorial's suite passes against the live Postgres:\n{printed}"
    );
    for test in [
        "posts::posts_round_trip",
        "posts::create_with_empty_title_returns_400",
        "posts::create_without_a_token_returns_401",
        "tutorial::the_token_route_the_tutorial_curls_opens_the_guarded_routes",
    ] {
        assert!(
            printed.contains(&format!("test {test} ... ok")),
            "{test} ran and passed:\n{printed}"
        );
    }
}
