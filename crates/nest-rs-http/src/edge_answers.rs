//! [`EdgeAnswers`] — what the edge answers a route's request with whatever the
//! route declares, for the route's document.

use crate::config::HttpConfig;
use crate::cors::HttpCors;

/// What the edge answers a route's request with under a config, whatever the
/// route declares: `Default`, nothing, is a document no transport serves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeAnswers {
    /// A request outrunning [`HttpConfig::request_timeout`] is answered `503`.
    pub request_timed_out: bool,
    /// A request body that stops arriving is answered `408`.
    pub body_stalled: bool,
    /// A request from an origin [`HttpConfig::cors`] does not admit is answered
    /// `403`.
    pub origin_refused: bool,
}

impl EdgeAnswers {
    /// What the edge answers under `config`.
    pub fn of(config: &HttpConfig) -> Self {
        Self {
            request_timed_out: config.request_timeout.is_some(),
            body_stalled: config.body_gap_fires(),
            origin_refused: config
                .cors
                .as_ref()
                .is_some_and(HttpCors::refuses_an_origin),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A gap reaching the budget never fires, which started first; with no
    /// budget it is the one bound.
    #[test]
    fn the_edge_answers_a_stalled_body_only_where_the_gap_can_fire_first() {
        let stalled = |budget: Option<u64>, gap: u64| {
            EdgeAnswers::of(&HttpConfig {
                request_timeout: budget.map(Duration::from_secs),
                body_read_timeout: Duration::from_secs(gap),
                ..HttpConfig::default()
            })
            .body_stalled
        };
        assert!(stalled(Some(20), 10));
        assert!(stalled(Some(11), 10));
        assert!(!stalled(Some(10), 10));
        assert!(!stalled(Some(5), 10));
        assert!(stalled(None, 10));
    }

    #[test]
    fn the_edge_answers_a_request_timeout_only_under_a_budget() {
        let timed_out = |budget: Option<Duration>| {
            EdgeAnswers::of(&HttpConfig {
                request_timeout: budget,
                ..HttpConfig::default()
            })
            .request_timed_out
        };
        assert!(timed_out(Some(Duration::from_secs(20))));
        assert!(!timed_out(None));
    }

    #[test]
    fn the_edge_refuses_an_origin_only_under_a_policy_admitting_less_than_every_one() {
        let refused = |origins: Option<&[&str]>| {
            EdgeAnswers::of(&HttpConfig {
                cors: origins.map(|origins| HttpCors {
                    origins: origins.iter().map(|origin| (*origin).to_owned()).collect(),
                    ..HttpCors::default()
                }),
                ..HttpConfig::default()
            })
            .origin_refused
        };
        assert!(refused(Some(&["https://app.example"])));
        assert!(!refused(Some(&["*"])));
        assert!(!refused(Some(&[])), "an empty list installs no layer");
        assert!(!refused(None));
    }
}
