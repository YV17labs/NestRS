//! The mirror join: what the CLI answers without linking the framework, run
//! beside the framework code it mirrors.
//!
//! `nestrs doctor` tells a developer what an app started in their shell makes
//! of a variable, and it links no framework crate — so that
//! `cargo install nest-rs-cli` stays independent of the version a project
//! pins. The answer is therefore a second implementation of the loader's, and
//! a second implementation drifts in silence: doctor answered `set` for a
//! `_FILE` naming an empty or a missing file and for a value given twice, which
//! the loader reads as unset or refuses at boot. Here both run over every shape
//! a deployment can give one variable, and must agree.
//!
//! **It reads no source**, so it declares nothing to the `blinds` join: it runs
//! the two implementations side by side.

use std::ffi::OsString;
use std::sync::Arc;

use crate::Followed;
use nest_rs_cli::{Resolution, resolve_variable};
use nest_rs_config::{ConfigService, MapSource, var_name};

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
    resolve_variable(real, "", &var_name(NAMESPACE, KEY))
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
