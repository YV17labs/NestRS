use nestrs_core::module;

use crate::interceptor::ServerTiming;

/// Server-Timing module — the crate's public entry point. Compose with
/// `#[module(imports = [ServerTimingModule, ...])]` to add the W3C
/// `Server-Timing` response header to every route. The interceptor
/// (`ServerTiming`, crate-private) is registered here, so an app activates it by
/// importing this module and never names the interceptor type.
#[module(providers = [ServerTiming])]
pub struct ServerTimingModule;
