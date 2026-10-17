//! Covers `src/unit.rs` from the site's side: the declaration each message and
//! connection hook emits, naming its unit and carrying the gateway's and its own
//! `#[meta]`.

use nest_rs_core::Posture;
use nest_rs_ws::{gateway, messages};

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

#[gateway(path = "/meta")]
#[meta(Audit("gateway"))]
#[meta(Cost(1))]
struct MeteredGateway;

#[messages]
impl MeteredGateway {
    #[subscribe_message("price")]
    #[public]
    #[meta(Cost(5))]
    async fn price(&self) {}

    #[on_connect]
    #[meta(Audit("joined"))]
    fn joined(&self) {}

    #[subscribe_message("idle")]
    #[public]
    #[meta(Unread)]
    async fn idle(&self) {}
}

#[test]
fn a_message_declares_its_unit_address_and_metadata() {
    let handler = MeteredGateway::__NESTRS_HANDLER_PRICE;
    assert_eq!(handler.unit(), nest_rs_ws::unit::MESSAGE);
    assert_eq!(handler.host_name(), "MeteredGateway");
    assert_eq!(handler.method(), "price");
    assert_eq!(handler.address(), "price", "the event a client sends");
    assert_eq!(handler.posture(), Posture::Public);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("gateway")));
}

#[test]
fn a_connection_hook_declares_its_own_unit_and_metadata() {
    let handler = MeteredGateway::__NESTRS_HANDLER_JOINED;
    assert_eq!(handler.unit(), nest_rs_ws::unit::CONNECT);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("joined")));
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(1)));
}
