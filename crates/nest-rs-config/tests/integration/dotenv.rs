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

/// What `main`'s first line finds: the kernel published the cascade before the
/// runtime existed, so a binary calls nothing for it. A child process, since
/// the cascade is read once per process, from where it runs.
mod published_before_main {
    use std::process::Command;

    const CHILD_ROLE: &str = "NEST_RS_CONFIG_CASCADE_CHILD";

    const CHILD_TEST: &str = "dotenv::published_before_main::cascade_child_process";

    #[nest_rs_core::main]
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[expect(
        clippy::disallowed_methods,
        reason = "what `std::env` holds on `main`'s first line is the assertion"
    )]
    async fn print_what_main_finds() {
        for name in ["CASCADE_PROBE", "CASCADE_DEPLOYED"] {
            println!("{name}={}", std::env::var(name).unwrap_or_default());
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent test hands the child its role through the environment"
    )]
    fn cascade_child_process() {
        if std::env::var(CHILD_ROLE).is_ok() {
            print_what_main_finds();
        }
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn main_reads_the_cascade_on_its_first_line_and_the_deployment_still_wins() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_PROBE=1\nCASCADE_DEPLOYED=from_file\n")?;

            let child = Command::new(std::env::current_exe().expect("the test binary"))
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .current_dir(jail.directory())
                .env(CHILD_ROLE, "1")
                .env("CASCADE_DEPLOYED", "from_deployment")
                .env_remove("CASCADE_PROBE")
                .output()
                .expect("the child runs");

            let stdout = String::from_utf8_lossy(&child.stdout);
            assert!(child.status.success(), "{stdout}");
            assert!(
                stdout.lines().any(|line| line == "CASCADE_PROBE=1"),
                "{stdout}"
            );
            assert!(
                stdout
                    .lines()
                    .any(|line| line == "CASCADE_DEPLOYED=from_deployment"),
                "the deployment's value wins over the file's: {stdout}",
            );
            Ok(())
        });
    }
}
