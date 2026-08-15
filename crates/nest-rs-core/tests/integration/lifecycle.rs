//! The fourth shape a lifecycle host can take, and the only one left at
//! runtime: bound as `dyn Trait`.
//!
//! [`ProviderResidency`](nest_rs_core::ProviderResidency) refuses the three shapes no
//! composition can fix (an edge host, `scope = request`, `scope = transient`)
//! at compile time. This one is the app's composition and the app can fix it:
//! `providers = [Foo as dyn Trait]` stores `Arc<dyn Trait>`, so nothing sits
//! under `Foo`, which is what the decorator resolves. Both halves are asserted
//! here — that it skips, and that listing the host both ways **constructs it
//! twice**. The second is why the skip line names causes and prescribes no
//! edit: double-listing is the repair it looks like, and is not one.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, hooks, injectable, module};

trait Bridge: Send + Sync {}

static DYN_ONLY: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct DynOnlyHost;

impl Bridge for DynOnlyHost {}

#[hooks]
impl DynOnlyHost {
    #[on_module_init]
    async fn init(&self) {
        DYN_ONLY.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [DynOnlyHost as dyn Bridge])]
struct DynOnlyModule;

/// Counts constructions. The first version of this file used a unit struct and
/// asserted only that the hook *ran*, which is true with one instance or two —
/// so it passed while claiming double-listing is a fix.
static BUILDS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct BothWaysHost {
    // Nothing reads it. `#[injectable]` builds a *unit* struct as a bare `Self`
    // and a named-field one through `Default`, so only the second can count.
    _counted: (),
}

impl Default for BothWaysHost {
    fn default() -> Self {
        BUILDS.fetch_add(1, Ordering::SeqCst);
        Self { _counted: () }
    }
}

impl Bridge for BothWaysHost {}

#[hooks]
impl BothWaysHost {
    #[on_module_init]
    async fn init(&self) {}
}

#[module(providers = [BothWaysHost, BothWaysHost as dyn Bridge])]
struct BothWaysModule;

#[tokio::test]
async fn a_host_bound_only_as_dyn_never_fires() {
    let app = App::new::<DynOnlyModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    assert_eq!(
        DYN_ONLY.load(Ordering::SeqCst),
        0,
        "the container holds `Arc<dyn Bridge>`, never a `DynOnlyHost` to call",
    );
}

/// **Why the skip line prescribes no edit.** Listing a host both ways looks like
/// the obvious repair and reads as one, but each binding runs the constructor:
/// the decorators fire on the concrete instance while every consumer injecting
/// `Arc<dyn Bridge>` holds a different one, and nothing anywhere says so —
/// `DuplicateProviderError` cannot fire, because the two container keys differ.
/// A hint that recommended this shipped for exactly one audit round.
#[tokio::test]
async fn listing_a_host_both_ways_builds_it_twice() {
    let app = App::new::<BothWaysModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        2,
        "each binding constructs its own instance — the decorators fire on one, \
         every `Arc<dyn Bridge>` consumer holds the other",
    );
}
