//! Covers `src/unit.rs` from the site's side: the declaration each listener
//! emits, naming its unit and carrying the impl block's and its own `#[meta]`.

use nest_rs_core::{Posture, injectable};
use nest_rs_events::listeners;

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

#[derive(Clone)]
struct Priced;

#[injectable]
#[derive(Default)]
struct MeteredListeners;

#[listeners]
#[meta(Audit("listeners"))]
#[meta(Cost(1))]
impl MeteredListeners {
    #[on_event]
    #[meta(Cost(5))]
    async fn on_priced(&self, _event: Priced) {}

    #[on_event]
    #[meta(Unread)]
    async fn on_unread(&self, _event: Priced) {}
}

#[injectable]
#[derive(Default)]
struct TariffListeners;

impl TariffListeners {
    const TRAIL: &'static str = "tariff";
    const BASE: u32 = 3;
}

#[listeners]
#[meta(Audit(Self::TRAIL))]
impl TariffListeners {
    #[on_event]
    #[meta(Cost(Self::BASE))]
    async fn on_priced(&self, _event: Priced) {}
}

#[test]
fn a_listener_declares_its_unit_address_and_metadata() {
    let handler = MeteredListeners::__NESTRS_HANDLER_ON_PRICED;
    assert_eq!(handler.unit(), nest_rs_events::unit::DISPATCH);
    assert_eq!(handler.host_name(), "MeteredListeners");
    assert_eq!(handler.method(), "on_priced");
    assert_eq!(handler.address(), "Priced", "the event it listens to");
    assert_eq!(handler.posture(), Posture::System);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("listeners")));
}

#[test]
fn a_value_names_its_host_through_self_on_the_block_and_the_method() {
    let reflector = TariffListeners::__NESTRS_HANDLER_ON_PRICED.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(3)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("tariff")));
}
