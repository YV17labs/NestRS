//! The root fields one operation dispatched, each by the key its response holds it
//! under: what resolves below a root field once its unit ended — a wire row's column
//! refusing — names that field as its `mounted` line does, by its site, never by the
//! alias a client wrote. An operation's unit also has them hold the errors the
//! operation reports, which async-graphql (`7.2.1`) keeps in its environment until the
//! answer is built, so an operation ending before it answers still files what they kept.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_graphql::{Context, ContextBase, Name, QueryEnv, QueryPathSegment, ServerError};
use nest_rs_core::__private::{RefusedAnswer, Site};
use nest_rs_core::ServedAddress;

use crate::filed_facts::first_in_operation;
use crate::operation::operation_label;

/// Held in each operation's data. Built by the first root field noted, so an
/// operation that dispatches none pays for one empty value.
#[derive(Default)]
pub(crate) struct RootFields {
    noted: OnceLock<Mutex<Noted>>,
    /// Whether it holds its operation's environment from the first root field
    /// noted: only where what built it takes the environment back
    /// ([`take_unanswered`](Self::take_unanswered)) however the operation
    /// ends, since the environment holds this value back.
    holds_errors: bool,
}

#[derive(Default)]
struct Noted {
    sites: Vec<(Name, &'static Site)>,
    env: Option<QueryEnv>,
}

impl RootFields {
    /// One that holds its operation's errors until its unit takes back what
    /// no answer carried.
    pub(crate) fn holding_errors() -> Self {
        Self {
            noted: OnceLock::new(),
            holds_errors: true,
        }
    }

    fn note(&self, key: &Name, site: &'static Site, env: &QueryEnv) {
        let mut noted = self
            .noted
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        noted.sites.push((key.clone(), site));
        if self.holds_errors && noted.env.is_none() {
            noted.env = Some(env.clone());
        }
    }

    /// The site of the root field held under the response key `key`.
    pub(crate) fn site(&self, key: &str) -> Option<&'static Site> {
        let noted = self
            .noted
            .get()?
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        noted
            .sites
            .iter()
            .find(|(noted, _)| noted.as_str() == key)
            .map(|(_, site)| *site)
    }

    /// Take the errors its operation reported that no answer carries — none
    /// once its answer is built, which takes them; what it met when cut,
    /// dropped or unwound first — letting go of the environment held for them.
    pub(crate) fn take_unanswered(&self) -> Vec<ServerError> {
        let Some(noted) = self.noted.get() else {
            return Vec::new();
        };
        let env = noted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .env
            .take();
        env.map_or_default(|env| {
            std::mem::take(&mut *env.errors.lock().unwrap_or_else(PoisonError::into_inner))
        })
    }
}

/// Note the root field `ctx` resolves as `site` dispatches it.
pub(crate) fn note_root(ctx: &Context<'_>, site: &'static Site) {
    if let Some(roots) = ctx.data_opt::<Arc<RootFields>>() {
        roots.note(&ctx.item.node.response_key().node, site, ctx.query_env);
    }
}

/// File `refused`, a wire row's refusal met resolving below the root field `root`
/// dispatched, once per operation, table and reason, naming that field as its
/// `mounted` line does. A root no unit dispatched (a schema built by hand) names
/// nothing a client wrote.
pub fn file_below_root<T>(
    ctx: &ContextBase<'_, T>,
    refused: &RefusedAnswer,
    root: Option<&'static Site>,
) {
    let field = root.map(|site| site.handler().address());
    let fact = format!(
        "judged {} {} {}",
        refused.entity(),
        refused.reason(),
        field.unwrap_or_default()
    );
    if first_in_operation(ctx, fact) {
        let kind = operation_label(ctx.query_env.operation.node.ty);
        let served = field.map_or_else(ServedAddress::default, |field| {
            ServedAddress::field(kind, field)
        });
        refused.file_at(root.map(Site::handler), served);
    }
}

/// The site of the root field the field `ctx` resolves sits under, where its
/// operation dispatched one.
pub fn root_site(ctx: &Context<'_>) -> Option<&'static Site> {
    let mut root = None;
    let mut node = ctx.path_node.as_ref();
    while let Some(at) = node {
        if let QueryPathSegment::Name(key) = at.segment {
            root = Some(key);
        }
        node = at.parent;
    }
    ctx.data_opt::<Arc<RootFields>>()?.site(root?)
}
