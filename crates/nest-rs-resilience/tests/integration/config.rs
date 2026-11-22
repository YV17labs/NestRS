//! Covers `src/config.rs`'s `timeout` and `retries` ranges against the copy
//! `#[resilient]` checks a method's pins with, since the macro crate cannot
//! depend on this one: both accept and refuse alike, over the ends the config
//! declares, and the decorator's refusal is the config's with the site in
//! front. Every other field is the policy's, and the decorator's copy of them
//! refuses each naming the variable the config reads it from.

use nest_rs_codegen::__private::resilient_policy_setting;
use nest_rs_config::{Config, ConfigService, Floor};
use nest_rs_resilience::ResilienceConfig;

/// Every field of `ResilienceConfig`, with the key its variable is read under
/// and the path code pins it through, read off the binding listed beside it.
/// A field added to the config fails to compile here until listed: list it,
/// never write `..`.
macro_rules! every_setting {
    ($($field:ident: $binding:ident),+ $(,)?) => {{
        let ResilienceConfig { $($field: _),+ } = ResilienceConfig::default();
        [$((
            stringify!($field),
            ResilienceConfig::$binding.key(),
            ResilienceConfig::$binding.field().path(),
        )),+]
    }};
}

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

/// A method pins its `timeout` and its `retries`; every other setting is the
/// policy's, for every caller, and `#[resilient]` refuses a key naming one with
/// where it is set: the variable and the field the config reads it through.
#[test]
fn every_other_setting_is_refused_at_a_method_naming_the_variable_that_sets_it() {
    let pins = [
        nest_rs_codegen::__private::TIMEOUT,
        nest_rs_codegen::__private::RETRIES,
    ];
    for (field, key, path) in every_setting!(
        timeout: TIMEOUT,
        retries: RETRIES,
        backoff_first: BACKOFF_FIRST,
        backoff_ceiling: BACKOFF_CEILING,
        breaker_window: BREAKER_WINDOW,
        breaker_failures: BREAKER_FAILURES,
        breaker_open: BREAKER_OPEN,
        breaker_age: BREAKER_AGE,
        breaker_probes: BREAKER_PROBES,
        concurrency: CONCURRENCY,
        concurrency_wait: CONCURRENCY_WAIT,
        concurrency_waiters: CONCURRENCY_WAITERS,
        rate_limit: RATE_LIMIT,
        rate_window: RATE_WINDOW,
        rate_wait: RATE_WAIT,
    ) {
        assert_eq!(
            path,
            format!("ResilienceConfig::{field}"),
            "the binding listed beside `{field}` is its own"
        );
        let refused = resilient_policy_setting(field);
        if pins.contains(&field) {
            assert_eq!(refused, None, "`{field}` is a pin the decorator takes");
            continue;
        }
        let refused = refused.unwrap_or_else(|| {
            panic!(
                "`{path}` has no answer at `#[resilient]` — give it a row in nest_rs_codegen's \
                 policy settings, or make it a pin the decorator takes"
            )
        });
        assert!(
            refused.starts_with(&format!("#[resilient] takes no `{field}`: ")),
            "{refused}"
        );
        assert!(
            refused.ends_with(&format!("`{key}`, or `{path}` in code")),
            "{refused}"
        );
    }
}
