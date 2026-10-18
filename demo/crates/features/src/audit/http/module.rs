use nest_rs::core::module;

use super::interceptor::AuditInterceptor;

#[module(providers = [AuditInterceptor])]
pub struct AuditHttpModule;
