//! Covers `src/reflector.rs` — a route's `#[meta]` values, the controller's and
//! the method's, read through the route's one declaration.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::{HandlerMetadata, Reflector, async_trait, controller, routes};
use poem::Request;

#[derive(Clone, Debug, PartialEq)]
struct Throttle(u32);

#[derive(Clone, Debug, PartialEq)]
struct Audit(&'static str);

static CLONED: AtomicUsize = AtomicUsize::new(0);

/// Counts every copy made of it.
#[derive(Debug)]
struct Counted(u32);

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONED.fetch_add(1, Ordering::SeqCst);
        Self(self.0)
    }
}

/// Inserts its own `Throttle` before the handler reads the declared one.
#[injectable]
#[derive(Default)]
struct Shadowing;

impl Layer for Shadowing {}

#[async_trait]
impl Guard for Shadowing {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        req.extensions_mut().insert(Throttle(1));
        req.extensions_mut().insert(Audit("inserted"));
        Ok(())
    }
}

impl HttpGuard for Shadowing {}

fn read(req: &Request) -> String {
    let reflector = Reflector::new(req);
    format!(
        "{:?} {:?} {:?}",
        reflector.get::<Throttle>(),
        reflector.get::<Audit>(),
        reflector.get::<Counted>().map(|counted| counted.0),
    )
}

#[controller(path = "/meta")]
#[use_guards(Shadowing)]
#[meta(Throttle(100))]
#[meta(Audit("controller"))]
struct MetaController;

impl MetaController {
    const QUOTA: u32 = 7;
}

#[routes]
impl MetaController {
    #[get("/declared")]
    #[meta(Throttle(10))]
    #[meta(Counted(3))]
    async fn declared(&self, req: &Request) -> String {
        read(req)
    }

    #[get("/inherited")]
    async fn inherited(&self, req: &Request) -> String {
        read(req)
    }

    #[get("/own")]
    #[meta(Throttle(Self::QUOTA))]
    async fn own(&self, req: &Request) -> String {
        read(req)
    }
}

#[module(providers = [Shadowing, MetaController])]
struct MetaModule;

#[tokio::test]
async fn the_methods_value_wins_and_a_host_value_of_another_type_survives() {
    let client = crate::boot::<MetaModule>().await;

    let declared = client.get("/meta/declared").send().await;
    declared.assert_status_is_ok();
    declared
        .assert_text(r#"Some(Throttle(10)) Some(Audit("controller")) Some(3)"#)
        .await;

    let inherited = client.get("/meta/inherited").send().await;
    inherited.assert_status_is_ok();
    inherited
        .assert_text(r#"Some(Throttle(100)) Some(Audit("controller")) None"#)
        .await;
}

#[tokio::test]
async fn a_methods_value_names_its_host_through_self() {
    let client = crate::boot::<MetaModule>().await;

    let own = client.get("/meta/own").send().await;
    own.assert_status_is_ok();
    own.assert_text(r#"Some(Throttle(7)) Some(Audit("controller")) None"#)
        .await;
}

#[tokio::test]
async fn a_value_a_guard_inserts_never_shadows_the_declared_one() {
    let client = crate::boot::<MetaModule>().await;

    let declared = client.get("/meta/declared").send().await;
    declared.assert_status_is_ok();
    let text = declared.0.into_body().into_string().await.expect("a body");
    assert!(
        text.starts_with("Some(Throttle(10))"),
        "the guard's `Throttle(1)` was read: {text}",
    );
}

#[tokio::test]
async fn a_declared_value_is_never_copied_per_request() {
    let client = crate::boot::<MetaModule>().await;
    let before = CLONED.load(Ordering::SeqCst);

    for _ in 0..3 {
        client
            .get("/meta/declared")
            .send()
            .await
            .assert_status_is_ok();
    }

    assert_eq!(CLONED.load(Ordering::SeqCst), before, "a value was cloned");
}
