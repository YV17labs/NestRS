//! Covers `src/unit.rs` from the site's side: the declaration each root field
//! emits, naming its unit and carrying the resolver's and its own `#[meta]`.

use nest_rs_core::Posture;
use nest_rs_graphql::{operations, resolver};

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

#[resolver]
#[meta(Audit("resolver"))]
#[meta(Cost(1))]
struct PricedResolver;

#[operations]
impl PricedResolver {
    #[query]
    #[public]
    #[meta(Cost(5))]
    async fn quote_price(&self) -> String {
        String::new()
    }

    #[mutation]
    #[public]
    async fn reprice(&self) -> bool {
        true
    }

    #[query]
    #[public]
    #[meta(Unread)]
    async fn idle(&self) -> bool {
        true
    }
}

#[test]
fn a_root_field_declares_its_unit_address_and_posture() {
    let handler = PricedResolver::__NESTRS_HANDLER_QUOTE_PRICE;
    assert_eq!(handler.unit(), nest_rs_graphql::unit::FIELD);
    assert_eq!(handler.host_name(), "PricedResolver");
    assert_eq!(handler.method(), "quote_price");
    assert_eq!(
        handler.address(),
        "quotePrice",
        "the field as the client writes it"
    );
    assert_eq!(handler.posture(), Posture::Public);
}

#[test]
fn a_root_field_reads_its_own_metadata_over_its_resolvers() {
    let priced = PricedResolver::__NESTRS_HANDLER_QUOTE_PRICE.reflector();
    assert_eq!(priced.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(priced.get::<Audit>(), Some(&Audit("resolver")));

    let repriced = PricedResolver::__NESTRS_HANDLER_REPRICE.reflector();
    assert_eq!(
        repriced.get::<Cost>(),
        Some(&Cost(1)),
        "the resolver's, inherited"
    );
}
