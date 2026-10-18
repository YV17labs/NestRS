//! The test doubles this crate's unit tests share.

use std::any::TypeId;

use nest_rs_core::__private::{
    GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers,
    declare_handler, declare_site,
};
use nest_rs_core::{Handler, Posture};

/// The host a hand-built entry names.
pub(crate) struct AudioProcessor;

static HANDLER: Handler = declare_handler(HandlerDeclaration {
    unit: crate::unit::JOB,
    host: TypeId::of::<AudioProcessor>(),
    host_name: "AudioProcessor",
    method: "run",
    address: "audio",
    posture: Posture::System,
    meta: MetaLevels::default,
});

/// The site a hand-built entry declares: no layer bound on it, every app-wide
/// one folded.
pub(crate) static SITE: Site = declare_site(
    &HANDLER,
    SiteDeclaration {
        layers: SiteLayers::default,
        global: GlobalLayers::ALL,
        guard_stage: None,
        class_gate: None,
    },
);
