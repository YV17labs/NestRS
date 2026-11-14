//! The routes a platform calls to ask whether the app lives — a health probe —
//! known to the edge before it opens the request's span, so a probe that
//! starts a trace starts it unsampled and its `2xx` line files at `debug`.
//!
//! A module declares its controller's routes probes by attaching
//! [`ProbeHost`] to it; `nest-rs-health` is the one caller. A probe is served
//! at a literal path, read off the route table as mounted, under the global
//! prefix.

use std::sync::Arc;

use http::Method;

/// Attached to a controller (`attach_meta::<C, _>(ProbeHost)`), every route it
/// mounts is a probe.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeHost;

/// Each probe's method and served path, which a request matches before any
/// route is.
#[derive(Clone, Debug, Default)]
pub(crate) struct Probes(Arc<[(Method, Box<str>)]>);

impl Probes {
    /// The probes `mounted`, a method and a literal served path each.
    pub(crate) fn new(mounted: Vec<(Method, String)>) -> Self {
        Self(
            mounted
                .into_iter()
                .map(|(method, path)| (method, path.into_boxed_str()))
                .collect(),
        )
    }

    /// Whether a request of `method` at `path` reaches a probe, as the router
    /// reads it: trailing slashes trimmed, and `HEAD` served by a `GET`.
    pub(crate) fn serves(&self, method: &Method, path: &str) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let path = crate::edge::canonical_path(path).unwrap_or(path);
        self.0.iter().any(|(probe, at)| {
            **at == *path && (probe == method || (*method == Method::HEAD && *probe == Method::GET))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn liveness() -> Probes {
        Probes::new(vec![(Method::GET, "/health/live".to_owned())])
    }

    #[test]
    fn a_probe_is_matched_as_the_router_reads_its_path() {
        let probes = liveness();
        for (method, path) in [
            (Method::GET, "/health/live"),
            (Method::GET, "/health/live/"),
            (Method::HEAD, "/health/live"),
        ] {
            assert!(probes.serves(&method, path), "{method} {path}");
        }
    }

    #[test]
    fn another_path_or_method_is_no_probe() {
        let probes = liveness();
        for (method, path) in [
            (Method::POST, "/health/live"),
            (Method::GET, "/health/live/x"),
            (Method::GET, "/health"),
            (Method::GET, "/health//live"),
        ] {
            assert!(!probes.serves(&method, path), "{method} {path}");
        }
        assert!(!Probes::default().serves(&Method::GET, "/health/live"));
    }
}
