use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_core::Container;
use poem::Route;

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata for a self-mounting HTTP endpoint owned by another
/// surface (a GraphQL schema, an MCP streamable-HTTP service). The closure
/// nests one opaque sub-endpoint at its own path; `path` and `label` exist
/// only so the transport can list the mount in its boot-time route log.
pub struct HttpEndpointMeta {
    path: Cow<'static, str>,
    label: Cow<'static, str>,
    mount: Arc<MountFn>,
}

impl HttpEndpointMeta {
    /// `path` and `label` accept either a `&'static str` or an owned `String`
    /// — so a module configured via `for_root` can nest at a runtime path.
    pub fn new<F>(
        path: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            path: path.into(),
            label: label.into(),
            mount: Arc::new(mount),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}
