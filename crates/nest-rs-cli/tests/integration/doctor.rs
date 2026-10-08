//! `nestrs doctor` — the toolchain report, and the variables it answers for.
//!
//! **Run with a cleared environment in an empty directory**: doctor reads its
//! shell and the `.env` cascade beside it. Doctor mirrors the loader without
//! linking it, so the parity tests at the end run both side by side (the loader
//! is a dev-dependency).

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use nest_rs_cli::{Resolution, cascade_refusals, resolve_variable};
use nest_rs_config::{ConfigService, Environment, MapSource, load_cascade, var_name};

use crate::harness::ENV_PREFIX_VAR;

/// `nestrs doctor` in `dir`, with nothing of this process's environment but
/// what finds `rustc` and `cargo`, and `vars` on top.
fn doctor(dir: &Path, vars: &[(&str, &str)]) -> Output {
    doctor_from(dir, None, vars)
}

/// `nestrs doctor`, run from `cwd` and examining `project` (`-p`) when given.
#[expect(
    clippy::disallowed_methods,
    reason = "the child inherits the suite's toolchain variables explicitly"
)]
fn doctor_from(cwd: &Path, project: Option<&Path>, vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nestrs"));
    command.arg("doctor").current_dir(cwd).env_clear();
    if let Some(project) = project {
        command.arg("-p").arg(project);
    }
    for toolchain in [
        "PATH",
        "HOME",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
    ] {
        if let Some(value) = std::env::var_os(toolchain) {
            command.env(toolchain, value);
        }
    }
    command
        .envs(vars.iter().copied())
        .output()
        .expect("doctor runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn doctor_passes_with_rust_toolchain() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let output = doctor(dir.path(), &[]);

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An unusable prefix is a blocking issue: every app started with it aborts on
/// the first name it builds.
#[test]
fn an_unusable_prefix_blocks() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let output = doctor(dir.path(), &[(ENV_PREFIX_VAR, "acme-1")]);

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(stdout(&output).contains("unusable"), "{}", stdout(&output));
}

/// A `_FILE` the loader cannot read aborts every app that reads the variable,
/// so doctor blocks; only the variable is printed, never the path.
#[test]
fn a_file_the_loader_cannot_read_blocks_naming_the_variable() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let missing = dir.path().join("secret-url");
    let file_var = "ACME_SEAORM__URL_FILE";
    let output = doctor(
        dir.path(),
        &[
            (ENV_PREFIX_VAR, "ACME"),
            (file_var, missing.to_str().expect("a UTF-8 path")),
        ],
    );

    let report = stdout(&output);
    assert!(!output.status.success(), "{report}");
    assert!(
        report.contains("ACME_SEAORM__URL: an app started here fails its boot")
            && report.contains(file_var)
            && !report.contains("secret-url"),
        "{report}"
    );
}

/// A `_FILE` naming a file holding only a line break is unset, as the loader
/// reads it — not `set`.
#[test]
fn a_file_holding_nothing_is_not_set() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let empty = dir.path().join("empty");
    std::fs::write(&empty, "\n").expect("write");
    let output = doctor(
        dir.path(),
        &[
            (ENV_PREFIX_VAR, "ACME"),
            (
                "ACME_REDIS__URL_FILE",
                empty.to_str().expect("a UTF-8 path"),
            ),
        ],
    );

    let report = stdout(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("ACME_REDIS__URL: not set"), "{report}");
}

/// An app started in the project opens a relative `_FILE` from the project, so
/// doctor examining it from anywhere else does too.
#[test]
fn a_relative_file_is_opened_from_the_project_examined_wherever_doctor_runs() {
    let project = tempfile::tempdir().expect("a project");
    let elsewhere = tempfile::tempdir().expect("another directory");
    std::fs::create_dir_all(project.path().join("secrets")).expect("secrets");
    std::fs::write(project.path().join("secrets/db"), "postgres://x\n").expect("write");
    std::fs::write(
        project.path().join(".env"),
        "NESTRS_SEAORM__URL_FILE=secrets/db\n",
    )
    .expect("write");

    let output = doctor_from(elsewhere.path(), Some(project.path()), &[]);
    let report = stdout(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("NESTRS_SEAORM__URL: set"), "{report}");
}

/// A `.env` naming the environment selector aborts every app started beside it,
/// so doctor blocks on it, naming the variable, not its value.
#[test]
fn a_cascade_naming_its_own_selector_blocks() {
    let dir = tempfile::tempdir().expect("a project");
    std::fs::write(dir.path().join(".env"), "NESTRS_ENV=production\n").expect("write");

    let output = doctor(dir.path(), &[]);
    let report = stdout(&output);
    assert!(!output.status.success(), "{report}");
    assert!(
        report.contains("NESTRS_ENV in the .env cascade: every app started here aborts")
            && !report.contains("production"),
        "{report}"
    );

    let restated = doctor(dir.path(), &[("NESTRS_ENV", "production")]);
    assert!(
        restated.status.success(),
        "a file restating the process's value is redundant, not wrong: {}",
        stdout(&restated)
    );
}

const NAMESPACE: &str = "mirror";
const KEY: &str = "URL";

/// What the loader makes of `KEY` in a process environment holding `vars` —
/// the same three answers doctor gives.
fn loader(vars: &[(String, String)]) -> Resolution {
    let source = MapSource::from_iter(vars.iter().cloned());
    match ConfigService::with_source(NAMESPACE, Arc::new(source)).setting(KEY) {
        Ok(Some(_)) => Resolution::Set,
        Ok(None) => Resolution::Unset,
        Err(refused) => Resolution::Refused(refused.to_string()),
    }
}

/// What doctor makes of the same variable in the same environment, with no
/// cascade.
fn mirrored(vars: &[(String, String)]) -> Resolution {
    let real = |name: &str| {
        vars.iter()
            .find(|(var, _)| var == name)
            .map(|(_, value)| OsString::from(value))
    };
    resolve_variable(real, "", &var_name(NAMESPACE, KEY), Path::new("/"))
}

#[test]
fn doctor_resolves_every_shape_of_a_variable_as_the_loader_does() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write the fixture");
        path.to_string_lossy().into_owned()
    };
    let value = file("value", b"redis://x\n");
    let crlf = file("crlf", b"redis://x\r\n\r\n");
    let breaks = file("breaks", b"\n\r\n");
    let empty = file("empty", b"");
    let not_utf8 = file("not-utf8", &[0xff, 0xfe, b'\n']);
    let huge = file("huge", &vec![b'x'; 1024 * 1024 + 1]);
    let missing = dir.path().join("missing").to_string_lossy().into_owned();
    let directory = dir.path().to_string_lossy().into_owned();
    let padded = format!("  {value}\n");

    let (inline, from_file) = (var_name(NAMESPACE, KEY), var_name(NAMESPACE, "URL_FILE"));
    let shapes: Vec<(&str, Vec<(&String, &str)>)> = vec![
        ("nothing", vec![]),
        ("an inline value", vec![(&inline, "redis://x")]),
        ("an empty inline value", vec![(&inline, "")]),
        ("a file holding a value", vec![(&from_file, &value)]),
        ("a file ending in CRLF", vec![(&from_file, &crlf)]),
        (
            "a file holding only line breaks",
            vec![(&from_file, &breaks)],
        ),
        ("an empty file", vec![(&from_file, &empty)]),
        ("a file that is not UTF-8", vec![(&from_file, &not_utf8)]),
        ("a file over a mebibyte", vec![(&from_file, &huge)]),
        ("a missing file", vec![(&from_file, &missing)]),
        ("a directory", vec![(&from_file, &directory)]),
        (
            "a path with whitespace around it",
            vec![(&from_file, &padded)],
        ),
        ("an empty path", vec![(&from_file, "")]),
        (
            "both spellings",
            vec![(&inline, "redis://x"), (&from_file, &value)],
        ),
        (
            "an empty inline beside a file",
            vec![(&inline, ""), (&from_file, &value)],
        ),
        (
            "an inline value beside an empty path",
            vec![(&inline, "redis://x"), (&from_file, "")],
        ),
    ];

    let mut disagreements = Vec::new();
    for (shape, vars) in &shapes {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(var, value)| ((*var).clone(), (*value).to_owned()))
            .collect();
        let (loader, doctor) = (loader(&vars), mirrored(&vars));
        let agree = matches!(
            (&loader, &doctor),
            (Resolution::Set, Resolution::Set)
                | (Resolution::Unset, Resolution::Unset)
                | (Resolution::Refused(_), Resolution::Refused(_))
        );
        if !agree {
            disagreements.push(format!("{shape}: the loader {loader:?}, doctor {doctor:?}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "`nestrs doctor` answers otherwise than the app it describes:\n{}",
        disagreements.join("\n"),
    );
}

/// A cascade naming one of the two variables that choose it is refused by the
/// loader whole — it panics at the first config read — and by doctor as a
/// blocking issue, over every shape: the selector with and without the
/// process's own value, an empty one, and the prefix restated or not.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail's closure signature is fixed"
)]
fn doctor_refuses_every_cascade_the_loader_refuses() {
    let selector = Environment::var_name();
    let prefix = nest_rs_core::EnvPrefix::current();
    // Another prefix than the process's, whichever prefix the suite runs under.
    let other = if prefix == "ACME" { "OTHER" } else { "ACME" };
    let shapes: Vec<(String, Option<&str>)> = vec![
        (format!("{selector}=production\n"), None),
        (format!("{selector}=production\n"), Some("production")),
        (format!("{selector}=production\n"), Some("test")),
        (format!("{selector}=\n"), Some("production")),
        (format!("{}={other}\n", nest_rs_core::EnvPrefix::VAR), None),
        (format!("{}={prefix}\n", nest_rs_core::EnvPrefix::VAR), None),
        ("UNRELATED=1\n".to_owned(), None),
    ];
    let mut disagreements = Vec::new();
    let mut refused_by_the_loader = 0usize;
    for (dotenv, process) in &shapes {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", dotenv)?;
            if let Some(value) = process {
                jail.set_env(&selector, value);
            }
            let loader = std::panic::catch_unwind(|| {
                load_cascade(Path::new("."), Environment::from_env());
            })
            .is_err();
            let real = |name: &str| {
                (name == selector)
                    .then_some(*process)
                    .flatten()
                    .map(OsString::from)
            };
            let doctor = !cascade_refusals(real, dotenv, prefix).is_empty();
            refused_by_the_loader += usize::from(loader);
            if loader != doctor {
                disagreements.push(format!(
                    "{dotenv:?} under {process:?}: the loader refuses {loader}, doctor {doctor}"
                ));
            }
            Ok(())
        });
    }
    assert!(
        disagreements.is_empty(),
        "`nestrs doctor` answers otherwise than the app it describes:\n{}",
        disagreements.join("\n"),
    );
    assert_eq!(
        refused_by_the_loader, 4,
        "the shapes refuse four times and pass three, or the two agree by never running"
    );
}
