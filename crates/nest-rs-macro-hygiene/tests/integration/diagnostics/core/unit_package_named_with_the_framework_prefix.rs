//! trybuild builds this fixture in a crate named `nest-rs-macro-hygiene-tests`:
//! a crate taking the framework's prefix that names a unit after itself is told
//! a package is `nestrs-<subject>`.

use nest_rs::core::operation_log::Unit;

const PROBE: Unit = nest_rs::core::unit!(
    "nest_rs_macro_hygiene_tests.probe",
    target: "nest_rs_macro_hygiene_tests",
    kind: Internal
);

fn main() {
    let _ = PROBE;
}
