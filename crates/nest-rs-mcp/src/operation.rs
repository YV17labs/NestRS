//! What one MCP operation *is*, and the guard chain that runs around it.

use std::fmt;

use nest_rs_core::__private::{ClassGate, Pending, new_unit_context};
use nest_rs_core::{Container, Handler};
use nest_rs_guards::__private::{
    Chain, GlobalBucket, SiteChainCell, SiteChainSources, failure_chain, run_stage,
};

use crate::McpError;
use crate::error::unresolvable_chain;
use crate::guard::denial_to_mcp_error;
use crate::view::McpView;

/// The container serving the current MCP operation, read off the ambient request
/// scope; `None` outside a dispatch nested under the HTTP transport edge.
pub fn current_container() -> Option<Container> {
    crate::scope::current_scope().map(|scope| scope.root().clone())
}

/// Which router an operation belongs to — the two roles `#[tools]` serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpOperationKind {
    /// A `#[tool]` method, reached by `tools/call`.
    Tool,
    /// A `#[prompt]` method, reached by `prompts/get`.
    Prompt,
}

impl McpOperationKind {
    /// The lowercase word this role is logged and labelled under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
        }
    }
}

impl fmt::Display for McpOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Run an operation site's guard chain, then its posture's class `gate` on the
/// same operation — called by `#[tools]` at the start of every decorated
/// operation, `handler` the site's declaration: the app-wide pool, the host's
/// `#[use_guards]` and the operation's own, each guard's `check` on the
/// operation; what they attached comes back, for the operation to run under.
///
/// The pool is part of this chain, as on `/graphql`: the endpoint's request
/// runs `check_http`, never `check`, so leaving the pool out here would leave a
/// global guard overriding only `check` never consulted. `cell` memoizes the
/// composed chain per app; `sources` is consulted only on the miss that
/// composes it.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one thing the decorator declared about its site; a struct would only rename them"
)]
pub async fn run_layered_mcp_chain(
    cell: &SiteChainCell,
    route_label: &'static str,
    host: &'static str,
    kind: McpOperationKind,
    name: &'static str,
    sources: &(dyn Fn() -> SiteChainSources + Sync),
    handler: &'static Handler,
    gate: Option<ClassGate>,
) -> Result<Pending, McpError> {
    let chain = match current_container() {
        Some(container) => cell
            .chain(&container, route_label, sources, |_| GlobalBucket::Fold)
            .map_err(denial_to_mcp_error)?,
        None => {
            // No app is ambient: declared guards cannot be resolved, so the
            // operation fails closed; one that declared none loses nothing.
            let declared = sources();
            if !declared.provider.is_empty() || !declared.method.is_empty() {
                return Err(unresolvable_chain(route_label));
            }
            Chain::from(Vec::new())
        }
    };
    if chain.is_empty() && gate.is_none() {
        return Ok(Pending::default());
    }

    let mut view = McpView::new(kind, name, host);
    let mut cx = new_unit_context(handler, &mut view, None, None);
    run_stage(&chain, gate, &mut cx).await.map_err(|refused| {
        denial_to_mcp_error(refused.into_denial(|guard, denial| {
            // Structural floor mirroring the HTTP edge's; host and operation stay
            // separate fields for an incident query.
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
                guard,
                host,
                kind = kind.as_str(),
                operation = name,
                status = denial.http_status(),
                error = failure_chain(denial),
                "guard denied the operation",
            );
        }))
    })
}

/// Whether an operation's description holds no prose — `str::trim` leaves
/// nothing of it. `const` so `#[tools]` can assert a non-literal description
/// (`include_str!`, a constant) at build time, which `str::trim` cannot.
pub const fn description_is_blank(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        // A `&str` is valid UTF-8, so the lead byte says how many continuation
        // bytes follow, and each carries six bits.
        let lead = bytes[at];
        let (mut code, width) = match lead {
            0x00..=0x7F => (lead as u32, 1),
            0xC0..=0xDF => ((lead & 0x1F) as u32, 2),
            0xE0..=0xEF => ((lead & 0x0F) as u32, 3),
            _ => ((lead & 0x07) as u32, 4),
        };
        let mut next = 1;
        while next < width {
            code = (code << 6) | (bytes[at + next] & 0x3F) as u32;
            next += 1;
        }
        match char::from_u32(code) {
            Some(character) if character.is_whitespace() => at += width,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::description_is_blank;

    /// `str::trim` is the oracle.
    #[test]
    fn a_description_is_blank_exactly_when_trim_leaves_nothing() {
        for character in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let alone = character.to_string();
            assert_eq!(
                description_is_blank(&alone),
                alone.trim().is_empty(),
                "{:?}",
                character,
            );
        }
        for text in [
            "",
            " ",
            "\t\n\r",
            "\u{3000}\u{a0}",
            " x ",
            "\u{3000}é\u{3000}",
            "😀",
        ] {
            assert_eq!(
                description_is_blank(text),
                text.trim().is_empty(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_check_runs_in_a_constant() {
        const {
            assert!(description_is_blank(concat!(" ", "\u{3000}")));
            assert!(!description_is_blank(concat!(" Counts ", "items.")));
        }
    }
}
