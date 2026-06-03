use nestrs_core::module;

use crate::interceptor::ServerTiming;

/// Add to `#[module(imports = [...])]` to attach the `Server-Timing` header on
/// every response.
#[module(providers = [ServerTiming])]
pub struct ServerTimingModule;
