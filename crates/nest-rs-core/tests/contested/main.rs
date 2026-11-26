//! Two `SafeLink`s claiming one error type, linked into one binary: the boot
//! refuses, naming the type and both sayers. Its own binary, since every app
//! booted beside the pair would be refused too.
#![allow(
    clippy::panic,
    reason = "a suite fails by panicking; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

use nest_rs_core::{App, ContestedSafeLinkError, SafeLink, error_message, module};

/// A library's error, quoting what it refused.
#[derive(Debug, thiserror::Error)]
#[error("partner call failed for {0}")]
struct PartnerError(&'static str);

/// What the library's driver says of it.
fn say_as_the_driver(_: &PartnerError) -> Option<String> {
    Some("partner call failed".to_owned())
}

/// What an app, claiming the library's type too, would say of it.
fn say_as_the_app(_: &PartnerError) -> Option<String> {
    Some("partner unavailable".to_owned())
}

nest_rs_core::inventory::submit! { SafeLink::of(&say_as_the_driver) }
nest_rs_core::inventory::submit! { SafeLink::of(&say_as_the_app) }

#[module]
struct EmptyModule;

#[tokio::test]
async fn two_sayers_of_one_type_refuse_the_boot_naming_both() {
    let Err(refused) = App::builder().module::<EmptyModule>().build().await else {
        panic!("one of the two sayers would be said in no line");
    };
    let contested = refused
        .downcast_ref::<ContestedSafeLinkError>()
        .unwrap_or_else(|| panic!("not the contested claim: {refused:#}"));

    assert_eq!(contested.link, std::any::type_name::<PartnerError>());
    let mut sayers = [contested.first, contested.second];
    sayers.sort_unstable();
    assert_eq!(
        sayers,
        ["contested::say_as_the_app", "contested::say_as_the_driver"]
    );
    let said = refused.to_string();
    for sayer in sayers {
        assert!(said.contains(sayer), "{said}");
    }
}

/// With no app built nothing refuses: a binary of that kind reads the first
/// entry linked, whichever it is.
#[test]
fn a_binary_building_no_app_reads_one_of_the_two() {
    let said = error_message(&PartnerError("sk_live_51H"));
    assert!(
        ["partner call failed", "partner unavailable"].contains(&said.as_str()),
        "{said}"
    );
}
