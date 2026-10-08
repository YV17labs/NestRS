//! The cascade's refusals around `<PREFIX>_ENV` — the variable that chooses
//! which `.env` files to read, and therefore can never usefully live in one.

use nest_rs_config::Environment;

/// Unset on the process, declared in the file: the file matches the resolution
/// by absence only, and still aborts.
#[test]
#[should_panic(expected = "is `development` in the `.env` cascade")]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
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

/// Set on the process to one thing, declared in the file as another: abort,
/// naming both values.
#[test]
#[should_panic(expected = "carries `production`")]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
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

/// Restating a value the process actually carries is redundant, not wrong.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
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
mod refusals_are_reported {
    use std::io::Write;

    use nest_rs_config::Environment;
    use nest_rs_testing::LogCapture;

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts what the cascade wrote into the process environment"
    )]
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
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_file_that_is_present_but_unreadable_is_reported_rather_than_skipped_silently() {
        figment::Jail::expect_with(|jail| {
            let logs = LogCapture::install();
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

/// A non-UTF-8 value in the real environment answers `None` and suppresses the
/// cascade, so the event is the only trace of it.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_non_utf8_environment_variable_is_reported_before_it_suppresses_the_cascade() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use nest_rs_testing::LogCapture;

    figment::Jail::expect_with(|jail| {
        let logs = LogCapture::install();
        jail.set_env(
            "FIXTURE_BROKEN_VALUE",
            OsStr::from_bytes(&[0xff, 0xfe]).to_string_lossy(),
        );
        // `set_env` round-trips through `String`, so the raw bytes go directly.
        // SAFETY: single-threaded test, and `Jail` restores the environment.
        #[expect(
            unsafe_code,
            reason = "the branch under test needs non-UTF-8 bytes, which only set_var writes; Jail restores the environment"
        )]
        unsafe {
            std::env::set_var("FIXTURE_BROKEN_VALUE", OsStr::from_bytes(&[0xff, 0xfe]))
        };

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
