//! Covers `src/config.rs`'s `timeout` and `retries` ranges against the copy
//! `#[resilient]` checks a method's pins with, since the macro crate cannot
//! depend on this one: both accept and refuse alike, over the ends the config
//! declares, and the decorator's refusal is the config's with the site in
//! front.

use nest_rs_config::{Config, ConfigService, Floor};
use nest_rs_resilience::ResilienceConfig;

/// The config read with the one variable `key` set to `value`, or what its
/// refusal says of the value, after the variable it names.
fn read(key: &str, value: &str) -> Result<ResilienceConfig, String> {
    ResilienceConfig::from_env(
        &ConfigService::with_vars("resilience", [(key, value)]),
        ResilienceConfig::default(),
    )
    .map_err(|refused| {
        let said = refused.to_string();
        let lead = format!(
            "invalid value for {}: ",
            nest_rs_config::var_name("resilience", key)
        );
        said.strip_prefix(&lead).unwrap_or(&said).to_owned()
    })
}

fn literal(written: &str) -> syn::Expr {
    syn::parse_str(written).expect("a literal")
}

#[test]
fn a_timeout_pin_is_held_to_the_range_of_the_configs_timeout() {
    let bounds = ResilienceConfig::TIMEOUT.bounds();
    let Floor::Units(floor) = bounds.least() else {
        panic!("the timeout's floor is a count of milliseconds");
    };
    let ceiling = bounds.most().count;
    for millis in [
        floor.count,
        2_000,
        ceiling - 1,
        ceiling,
        ceiling + 1,
        ceiling * 6,
    ] {
        let runtime = read(&ResilienceConfig::TIMEOUT.key(), &millis.to_string());
        let decorator =
            nest_rs_codegen::__private::resilient_timeout(&literal(&format!("\"{millis}ms\"")));
        match (runtime, decorator) {
            (Ok(read), Ok(pinned)) => {
                assert_eq!(read.timeout.as_millis(), u128::from(pinned));
            }
            (Err(runtime), Err(decorator)) => assert_eq!(
                decorator.to_string(),
                format!("#[resilient] `timeout`: {runtime}"),
            ),
            (runtime, decorator) => {
                panic!("the two copies disagree on {millis} ms: {runtime:?} and {decorator:?}")
            }
        }
    }
    assert_eq!(
        floor.count, 1,
        "a duration literal holds a millisecond at least, so the decorator refuses below it by \
         its grammar"
    );
    assert!(read(&ResilienceConfig::TIMEOUT.key(), "0").is_err());
    assert!(nest_rs_codegen::__private::resilient_timeout(&literal("\"0ms\"")).is_err());
}

#[test]
fn a_retries_pin_is_held_to_the_range_of_the_configs_retries() {
    let bounds = ResilienceConfig::RETRIES.bounds();
    let (floor, ceiling) = (bounds.least().count, bounds.most().count);
    for count in [floor, 3, ceiling - 1, ceiling, ceiling + 1, ceiling * 10] {
        let runtime = read(&ResilienceConfig::RETRIES.key(), &count.to_string());
        let decorator = nest_rs_codegen::__private::resilient_retries(&literal(&count.to_string()));
        match (runtime, decorator) {
            (Ok(read), Ok(pinned)) => {
                assert_eq!(
                    read.retries,
                    Some(usize::try_from(pinned).expect("a count"))
                );
            }
            (Err(runtime), Err(decorator)) => assert_eq!(
                decorator.to_string(),
                format!(
                    "#[resilient] `retries`: {}",
                    runtime.replace(", or 0 to turn it off", "")
                ),
                "the variable's own off is no pin's",
            ),
            (runtime, decorator) => {
                panic!("the two copies disagree on {count}: {runtime:?} and {decorator:?}")
            }
        }
    }
    let off = read(&ResilienceConfig::RETRIES.key(), "0").expect("`0` is the variable's off");
    assert_eq!(off.retries, None);
    let refused = nest_rs_codegen::__private::resilient_retries(&literal("0"))
        .expect_err("a method attempted once declares no `idempotent`")
        .to_string();
    assert!(refused.contains("`0` turns retries off"), "{refused}");
}
