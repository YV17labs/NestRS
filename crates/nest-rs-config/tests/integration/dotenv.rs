//! The cascade's refusals around `<PREFIX>_ENV` — the variable that chooses
//! which `.env` files to read, and therefore can never usefully live in one.
//!
//! The stakes are higher than redundancy: `Environment::init` publishes the
//! cascade into the process env (set-if-absent), so a committed
//! `NESTRS_ENV=development` would become process-visible and flip
//! `Environment::declared()` from `None` to `Some(Development)` — arming every
//! development-only affordance on any deployment that left the variable unset.
//! Found by booting a scaffolded app with exactly that file, not by reading.

use nest_rs_config::Environment;

/// Unset on the process, declared in the file: the laundering case. The
/// cascade defaulted to `development` *by absence*, so the file "matches" the
/// resolved environment — which is precisely why matching the resolution is
/// not the tolerance. Abort.
#[test]
#[should_panic(expected = "is `development` in the `.env` cascade")]
#[allow(clippy::result_large_err)]
fn the_environment_written_into_the_cascade_aborts() {
    figment::Jail::expect_with(|jail| {
        jail.create_file(
            ".env",
            &format!("{}=development\n", Environment::var_name()),
        )?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
        Ok(())
    });
}

/// Set on the process to one thing, declared in the file as another: the file
/// is contradicting the deployment, and set-if-absent publishing would keep
/// the contradiction invisible. Abort, naming both values.
#[test]
#[should_panic(expected = "carries `production`")]
#[allow(clippy::result_large_err)]
fn the_environment_contradicted_by_the_cascade_aborts() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(Environment::var_name(), "production");
        jail.create_file(
            ".env",
            &format!("{}=development\n", Environment::var_name()),
        )?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Production);
        Ok(())
    });
}

/// Restating a value the process actually carries is redundant, not wrong —
/// the same tolerance the prefix refusal grants, and the only one this check
/// can grant safely: with the variable genuinely set, set-if-absent publishing
/// has nothing to launder.
#[test]
#[allow(clippy::result_large_err)]
fn the_environment_restated_by_the_cascade_is_tolerated() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(Environment::var_name(), "development");
        jail.create_file(
            ".env",
            &format!("{}=development\n", Environment::var_name()),
        )?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
        Ok(())
    });
}

/// A value the cascade drops leaves nothing behind but the event.
///
/// Every refusal in `merge_file` is `warn` + return: the key is simply absent
/// afterwards, indistinguishable from one nobody wrote. So the operator reading
/// "why is my secret unset" has these lines and no other trace, which is why a
/// bare one would be a defect rather than a style nit.
mod refusals_are_reported {
    use std::io::Write;

    use nest_rs_config::Environment;
    use nest_rs_testing::LogCapture;

    #[test]
    #[allow(clippy::result_large_err)]
    fn a_malformed_line_is_counted_and_named_with_its_file() {
        figment::Jail::expect_with(|jail| {
            let logs = LogCapture::install();
            jail.create_file(
                ".env",
                "GOOD_ONE=kept\nno-equals-here\nalso-missing-an-equals\n",
            )?;
            nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
            assert_eq!(std::env::var("GOOD_ONE").unwrap(), "kept");

            let event = logs.expect_one("nest_rs::config", "skipped malformed .env lines");
            assert_eq!(event.level, "warn");
            // One aggregate event per file, carrying the count — a per-line
            // event would bury a real cascade under its own noise.
            assert_eq!(event.field("skipped").as_deref(), Some("2"));
            assert!(
                event.field("path").is_some_and(|p| p.contains(".env")),
                "the event names the file, got {:?}",
                event.fields,
            );
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn a_file_that_is_present_but_unreadable_is_reported_rather_than_skipped_silently() {
        figment::Jail::expect_with(|jail| {
            let logs = LogCapture::install();
            // Present and non-UTF-8: `read_to_string` fails with `InvalidData`,
            // which is the branch that separates "no such file" (the normal
            // case, silent) from "a file we could not read".
            let path = std::path::Path::new(".env");
            let mut file = std::fs::File::create(path).expect("create the .env");
            file.write_all(&[b'A', b'=', 0xff, 0xfe, b'\n'])
                .expect("write invalid UTF-8");
            drop(file);

            nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);

            let event = logs.expect_one("nest_rs::config", "skipping unreadable .env file");
            assert_eq!(event.level, "warn");
            assert!(
                event.field("error").is_some(),
                "the event carries the io error, got {:?}",
                event.fields,
            );
            let _ = jail;
            Ok(())
        });
    }
}

/// A non-UTF-8 value in the real environment.
///
/// It is the one case where the variable is genuinely *present* and still
/// answers `None`: the cascade is suppressed as well, so a `.env` carrying a
/// usable value for the same key is deliberately not consulted. Nothing about
/// that is visible to the caller — the field simply falls back to its default —
/// which is what makes the event the only place the mistake exists.
#[test]
#[allow(clippy::result_large_err)]
fn a_non_utf8_environment_variable_is_reported_before_it_suppresses_the_cascade() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use nest_rs_testing::LogCapture;

    figment::Jail::expect_with(|jail| {
        let logs = LogCapture::install();
        // `Jail` restores the process env on drop, which is what makes writing
        // a deliberately broken value safe here.
        jail.set_env(
            "FIXTURE_BROKEN_VALUE",
            OsStr::from_bytes(&[0xff, 0xfe]).to_string_lossy(),
        );
        // `set_env` round-trips through `String`, so write the raw bytes
        // directly — the lossy form above is valid UTF-8 and would not reach
        // the branch under test.
        // SAFETY: single-threaded test, and `Jail` restores the environment.
        unsafe { std::env::set_var("FIXTURE_BROKEN_VALUE", OsStr::from_bytes(&[0xff, 0xfe])) };

        assert_eq!(
            nest_rs_config::env_var("FIXTURE_BROKEN_VALUE"),
            None,
            "a value that is not a string is treated as unset",
        );

        let event = logs.expect_one(
            "nest_rs::config",
            "environment variable is not valid UTF-8 — treated as unset, cascade suppressed",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("name").as_deref(), Some("FIXTURE_BROKEN_VALUE"));
        Ok(())
    });
}
