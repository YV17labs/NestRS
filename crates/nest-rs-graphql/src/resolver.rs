//! Runtime schema composition from a link-time resolver registry.
//!
//! `#[resolver]` splits its `#[query]`/`#[mutation]` methods into generated
//! `#[Object]` structs and submits each to the [`inventory`] registry. The
//! roots [`DiscoveredQuery`] / [`DiscoveredMutation`] are static types whose
//! fields are merged from the registry at build time — the runtime analog of
//! async-graphql's compile-time `MergedObject`.
//!
//! Module-gating filters the inventory by access-graph reachability. Because
//! `create_type_info` / `is_empty` are static methods async-graphql calls
//! during `Schema::build` (no container access), the reachable set lives in a
//! thread-local installed by [`build_schema`] for the build's duration.

use std::any::TypeId;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::Field;
use async_graphql::registry::{MetaType, MetaTypeId, Registry};
use async_graphql::{
    CacheControl, ContainerType, Context, ContextSelectionSet, EmptySubscription, ObjectType,
    OutputType, Positioned, SDLExportOptions, Schema, ServerResult, Value,
};
use nest_rs_core::{Container, ReachableProviders};

/// Which root a resolver's methods contribute to.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphqlResolverKind {
    Query,
    Mutation,
}

/// Object-safe view of a code-first resolver. `ContainerType`/`OutputType`
/// aren't object-safe (static `type_name`/`create_type_info`), so the runtime
/// roots store members behind this boxed-future shim. Blanket-impl'd for
/// every `#[Object]` type.
#[doc(hidden)]
pub trait GraphqlResolverObject: Send + Sync {
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>>;
}

impl<T: ContainerType + Send + Sync> GraphqlResolverObject for T {
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>> {
        Box::pin(ContainerType::resolve_field(self, ctx))
    }
}

/// One generated resolver object, submitted by `#[resolver]`.
/// `resolver_type_id` keys the entry against [`ReachableProviders`] for
/// module-gating.
#[doc(hidden)]
pub struct GraphqlResolverRegistration {
    pub kind: GraphqlResolverKind,
    pub resolver_type_id: fn() -> TypeId,
    pub type_info: fn(&mut Registry) -> MetaType,
    pub build: fn(&Container) -> Box<dyn GraphqlResolverObject>,
}

inventory::collect!(GraphqlResolverRegistration);

thread_local! {
    // Reachable provider `TypeId`s installed by [`build_schema`] for the
    // build's duration. `None` => no gating (bare `Schema::build` outside
    // our flow includes every linked resolver).
    static REACHABLE: RefCell<Option<Arc<HashSet<TypeId>>>> = const { RefCell::new(None) };
}

fn is_member_active(reg: &GraphqlResolverRegistration) -> bool {
    REACHABLE.with(|cell| match &*cell.borrow() {
        Some(set) => set.contains(&(reg.resolver_type_id)()),
        None => true,
    })
}

fn kind_has_members(kind: GraphqlResolverKind) -> bool {
    inventory::iter::<GraphqlResolverRegistration>().any(|reg| reg.kind == kind && is_member_active(reg))
}

fn build_members(container: &Container, kind: GraphqlResolverKind) -> Vec<Box<dyn GraphqlResolverObject>> {
    inventory::iter::<GraphqlResolverRegistration>()
        .filter(|reg| reg.kind == kind && is_member_active(reg))
        .map(|reg| (reg.build)(container))
        .collect()
}

/// Merge fields of every registered object of `kind` into one root object.
/// Member object types register as a side effect of `create_fake_output_type`
/// but go unreferenced, so `remove_unused_types` drops them — only the merged
/// root remains in the SDL.
fn merge_type_info<T: OutputType>(
    registry: &mut Registry,
    kind: GraphqlResolverKind,
    type_name: &str,
) -> String {
    registry.create_output_type::<T, _>(MetaTypeId::Object, |registry| {
        let mut fields = IndexMap::new();
        for reg in inventory::iter::<GraphqlResolverRegistration>() {
            if reg.kind != kind || !is_member_active(reg) {
                continue;
            }
            if let MetaType::Object {
                fields: member_fields,
                ..
            } = (reg.type_info)(registry)
            {
                fields.extend(member_fields);
            }
        }
        MetaType::Object {
            name: type_name.to_string(),
            description: None,
            fields,
            cache_control: CacheControl::default(),
            extends: false,
            shareable: false,
            resolvable: true,
            keys: None,
            visible: None,
            inaccessible: false,
            interface_object: false,
            tags: Default::default(),
            is_subscription: false,
            rust_typename: Some(std::any::type_name::<T>()),
            directive_invocations: Default::default(),
            requires_scopes: Default::default(),
        }
    })
}

macro_rules! discovered_root {
    ($name:ident, $kind:expr_2021, $type_name:literal) => {
        pub(crate) struct $name {
            members: Vec<Box<dyn GraphqlResolverObject>>,
        }

        impl $name {
            fn from_registry(container: &Container) -> Self {
                Self {
                    members: build_members(container, $kind),
                }
            }
        }

        impl OutputType for $name {
            fn type_name() -> Cow<'static, str> {
                Cow::Borrowed($type_name)
            }

            fn create_type_info(registry: &mut Registry) -> String {
                merge_type_info::<Self>(registry, $kind, $type_name)
            }

            async fn resolve(
                &self,
                _ctx: &ContextSelectionSet<'_>,
                _field: &Positioned<Field>,
            ) -> ServerResult<Value> {
                unreachable!("object root resolves through resolve_field")
            }
        }

        impl ContainerType for $name {
            fn is_empty() -> bool {
                !kind_has_members($kind)
            }

            async fn resolve_field(&self, ctx: &Context<'_>) -> ServerResult<Option<Value>> {
                for member in &self.members {
                    if let Some(value) = member.resolve_field(ctx).await? {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
        }

        impl ObjectType for $name {}
    };
}

discovered_root!(DiscoveredQuery, GraphqlResolverKind::Query, "Query");
discovered_root!(DiscoveredMutation, GraphqlResolverKind::Mutation, "Mutation");

/// Build the discovered schema. Subscriptions are not yet supported.
///
/// Installs [`ReachableProviders`] in [`REACHABLE`] for the duration of
/// `Schema::build`. The drop guard restores the previous value even on panic —
/// a leak would otherwise carry one test's reachable set into another's
/// build on the same thread.
pub(crate) fn build_schema(
    container: Container,
) -> Schema<DiscoveredQuery, DiscoveredMutation, EmptySubscription> {
    let reachable = container
        .get::<ReachableProviders>()
        .map(|p| Arc::new(p.0.clone()));
    let _reset = ReachableResetGuard::set(reachable);
    Schema::build(
        DiscoveredQuery::from_registry(&container),
        DiscoveredMutation::from_registry(&container),
        EmptySubscription,
    )
    .data(container.clone())
    .extension(crate::loader::LoaderExtensionFactory::new(container))
    .finish()
}

/// RAII swap on [`REACHABLE`]: install on construction, restore (not clear) on
/// drop — so a nested build cannot strand the outer build's set.
struct ReachableResetGuard(Option<Arc<HashSet<TypeId>>>);

impl ReachableResetGuard {
    fn set(new: Option<Arc<HashSet<TypeId>>>) -> Self {
        let previous = REACHABLE.with(|cell| cell.replace(new));
        Self(previous)
    }
}

impl Drop for ReachableResetGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        REACHABLE.with(|cell| *cell.borrow_mut() = previous);
    }
}

/// Render the composed schema as SDL. Types, fields, arguments, and enum
/// values are sorted: the resolver registry's link-time iteration order is
/// not stable, and would otherwise churn the committed SDL diff.
pub(crate) fn render_sdl(
    schema: &Schema<DiscoveredQuery, DiscoveredMutation, EmptySubscription>,
) -> String {
    schema.sdl_with_options(
        SDLExportOptions::new()
            .sorted_fields()
            .sorted_arguments()
            .sorted_enum_items(),
    )
}
