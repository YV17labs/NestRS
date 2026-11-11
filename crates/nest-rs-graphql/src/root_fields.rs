//! The root fields one operation dispatched, each by the key its response holds it
//! under: what resolves below a root field once its unit ended — a wire row's column
//! refusing — names that field as its `mounted` line does, by its site, never by the
//! alias a client wrote.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_graphql::{Context, ContextBase, Name, QueryPathSegment};
use nest_rs_core::__private::{RefusedAnswer, Site};
use nest_rs_core::ServedAddress;

use crate::filed_facts::first_in_operation;
use crate::operation::operation_label;

/// Held in each operation's data. Built by the first root field noted, so an
/// operation that dispatches none pays for one empty value.
#[derive(Default)]
pub(crate) struct RootFields(OnceLock<Mutex<Vec<(Name, &'static Site)>>>);

impl RootFields {
    fn note(&self, key: &Name, site: &'static Site) {
        self.0
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((key.clone(), site));
    }

    fn site(&self, key: &str) -> Option<&'static Site> {
        let roots = self.0.get()?.lock().unwrap_or_else(PoisonError::into_inner);
        roots
            .iter()
            .find(|(noted, _)| noted.as_str() == key)
            .map(|(_, site)| *site)
    }
}

/// Note the root field `ctx` resolves as `site` dispatches it.
pub(crate) fn note_root(ctx: &Context<'_>, site: &'static Site) {
    if let Some(roots) = ctx.data_opt::<Arc<RootFields>>() {
        roots.note(&ctx.item.node.response_key().node, site);
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
