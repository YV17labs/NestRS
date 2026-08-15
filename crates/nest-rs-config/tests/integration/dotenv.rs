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
        jail.create_file(".env", "NESTRS_ENV=development\n")?;
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
        jail.set_env("NESTRS_ENV", "production");
        jail.create_file(".env", "NESTRS_ENV=development\n")?;
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
        jail.set_env("NESTRS_ENV", "development");
        jail.create_file(".env", "NESTRS_ENV=development\n")?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
        Ok(())
    });
}
