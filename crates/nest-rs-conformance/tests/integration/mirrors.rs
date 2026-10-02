//! The mirror join: what the CLI answers without linking the framework, run
//! beside the framework code it mirrors.
//!
//! `nestrs doctor` tells a developer what an app started in their shell makes
//! of a variable, and of the `.env` cascade, and it links no framework crate —
//! so that `cargo install nest-rs-cli` stays independent of the version a
//! project pins. The answer is therefore a second implementation of the
//! loader's, and a second implementation drifts in silence: doctor answered
//! `set` for a `_FILE` naming an empty or a missing file and for a value given
//! twice, which the loader reads as unset or refuses at boot; it opened a
//! relative `_FILE` from its own directory rather than the app's; and it called
//! healthy a cascade naming its own selector, which aborts every app. Here both
//! run over every shape a deployment can give one variable, and every cascade
//! naming a selector, and must agree.
//!
//! **It reads no source**, so it declares nothing to the `blinds` join: it runs
//! the two implementations side by side.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use crate::Followed;
use nest_rs_cli::{Resolution, cascade_refusals, resolve_variable};
use nest_rs_config::{ConfigService, Environment, MapSource, load_cascade, var_name};

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    Vec::new()
}

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
fn doctor(vars: &[(String, String)]) -> Resolution {
    let real = |name: &str| {
        vars.iter()
            .find(|(var, _)| var == name)
            .map(|(_, value)| OsString::from(value))
    };
    resolve_variable(real, "", &var_name(NAMESPACE, KEY), Path::new("/"))
}

const NAMESPACE: &str = "mirror";
const KEY: &str = "URL";

#[test]
fn doctor_resolves_every_shape_of_a_variable_as_the_loader_does() {
    let dir = std::env::temp_dir().join(format!("nest-rs-mirror-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write the fixture");
        path.to_string_lossy().into_owned()
    };
    let value = file("value", b"redis://x\n");
    let crlf = file("crlf", b"redis://x\r\n\r\n");
    let breaks = file("breaks", b"\n\r\n");
    let empty = file("empty", b"");
    let not_utf8 = file("not-utf8", &[0xff, 0xfe, b'\n']);
    let huge = file("huge", &vec![b'x'; 1024 * 1024 + 1]);
    let missing = dir.join("missing").to_string_lossy().into_owned();
    let directory = dir.to_string_lossy().into_owned();
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
        let (loader, doctor) = (loader(&vars), doctor(&vars));
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
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        disagreements.is_empty(),
        "`nestrs doctor` answers otherwise than the app it describes:\n{}",
        disagreements.join("\n"),
    );
}

/// A relative `_FILE` is opened from the directory the app is started in: the
/// loader from its working directory, doctor from the one it examines.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn doctor_opens_a_relative_file_where_the_loader_does() {
    figment::Jail::expect_with(|jail| {
        jail.create_dir("secrets")?;
        jail.create_file("secrets/url", "redis://x\n")?;
        let from_file = var_name(NAMESPACE, "URL_FILE");
        for path in ["secrets/url", "secrets/missing"] {
            let vars = [(from_file.clone(), path.to_owned())];
            let loader = loader(&vars);
            let real = |name: &str| (name == from_file).then(|| OsString::from(path));
            let doctor = resolve_variable(real, "", &var_name(NAMESPACE, KEY), jail.directory());
            assert!(
                std::mem::discriminant(&loader) == std::mem::discriminant(&doctor),
                "{path}: the loader {loader:?}, doctor {doctor:?}"
            );
        }
        Ok(())
    });
}

/// A cascade naming one of the two variables that choose it is refused by the
/// loader whole — it panics at the first config read — and by doctor as a
/// blocking issue, over every shape: the selector with and without the
/// process's own value, an empty one, and the prefix restated or not.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn doctor_refuses_every_cascade_the_loader_refuses() {
    let selector = Environment::var_name();
    let prefix = nest_rs_core::EnvPrefix::current();
    let shapes: Vec<(String, Option<&str>)> = vec![
        (format!("{selector}=production\n"), None),
        (format!("{selector}=production\n"), Some("production")),
        (format!("{selector}=production\n"), Some("test")),
        (format!("{selector}=\n"), Some("production")),
        (format!("{}=ACME\n", nest_rs_core::EnvPrefix::VAR), None),
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
