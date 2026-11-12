use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::interceptor::AuditInterceptor;

#[module(imports = [HttpModule], providers = [AuditInterceptor])]
pub struct AuditHttpModule;
