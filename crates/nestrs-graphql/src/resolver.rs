//! Runtime schema composition from a link-time resolver registry.
//!
//! `#[resolver]` on an impl block splits its `#[query]`/`#[mutation]` methods
//! into generated `#[Object]` structs and submits each to the [`inventory`]
//! registry below. The schema then composes itself at boot — no central
//! `queries = [...]` list, no `main.rs` wiring.
//!
//! The trick: async-graphql's roots are static types (`Schema<Q, M, S>`), but
//! `Q`/`M` here are *our* types ([`DiscoveredQuery`]/[`DiscoveredMutation`])
//! whose fields are merged from the registry. `create_type_info` (static) reads
//! the registry to merge each member's fields under one root type; `is_empty`
//! reads it to behave like `EmptyMutation` when nothing registered;
//! `resolve_field` (instance) dispatches over the members built from the
//! container. This mirrors what async-graphql's own `MergedObject` does over a
//! compile-time tuple, but driven by the registry at runtime.
//!
//! Module-gating filters the inventory by access-graph reachability: only
//! resolvers whose `TypeId` is in [`ReachableProviders`] (read from the
//! container at schema-build time) end up in the schema. `create_type_info`
//! and `is_empty` are static methods async-graphql calls during
//! `Schema::build`, so they cannot receive the container — the reachable set
//! lives in a thread-local installed by [`build_schema`] for the duration of
//! the build and read by [`is_member_active`].

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
use nestrs_core::{Container, ReachableProviders};

/// Which root a resolver's methods contribute to. Set per method by
/// `#[query]` / `#[mutation]`; carried on the [`ResolverRegistration`].
///
/// `pub` only so `#[resolver]`-generated code can name it; not app-facing.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverKind {
    Query,
    Mutation,
}

/// Object-safe view of a code-first resolver. `ContainerType`/`OutputType`
/// aren't object-safe (static `type_name`/`create_type_info`), so the runtime
/// roots store members behind this boxed-future shim instead. Blanket-impl'd
/// for every `#[Object]` type, so `#[resolver]` boxes its generated objects
/// without the app seeing this trait.
#[doc(hidden)]
pub trait ResolverObject: Send + Sync {
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>>;
}

impl<T: ContainerType + Send + Sync> ResolverObject for T {
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>> {
        Box::pin(ContainerType::resolve_field(self, ctx))
    }
}

/// One generated resolver object, submitted to the [`inventory`] registry by
/// `#[resolver]`. `type_info` contributes the object's fields to the schema
/// registry (it closes over the concrete type via
/// `Registry::create_fake_output_type`); `build` constructs the resolver from
/// the container at schema-build time. `resolver_type_id` is the resolver
/// struct's `TypeId` — the container key it provides — so the schema build
/// can module-gate this entry by [`ReachableProviders`].
#[doc(hidden)]
pub struct ResolverRegistration {
    pub kind: ResolverKind,
    pub resolver_type_id: fn() -> TypeId,
    pub type_info: fn(&mut Registry) -> MetaType,
    pub build: fn(&Container) -> Box<dyn ResolverObject>,
}

inventory::collect!(ResolverRegistration);

thread_local! {
    /// The reachable provider `TypeId`s for the schema being built on this
    /// thread — installed by [`build_schema`] for the duration of the build
    /// and read by [`is_member_active`] from the static `OutputType` methods
    /// async-graphql calls (`create_type_info`, `is_empty`). A schema build
    /// is synchronous and single-threaded, so a thread-local fits without
    /// risking cross-build leakage. `None` here means no gating (a bare
    /// `Schema::build` called outside our flow) — the prior behaviour, every
    /// linked resolver included.
    static REACHABLE: RefCell<Option<Arc<HashSet<TypeId>>>> = const { RefCell::new(None) };
}

/// True when this registration's resolver is reachable from the running app's
/// module tree — every `inventory::iter` site below funnels through this so
/// the schema sees only the resolvers an app composes.
fn is_member_active(reg: &ResolverRegistration) -> bool {
    REACHABLE.with(|cell| match &*cell.borrow() {
        Some(set) => set.contains(&(reg.resolver_type_id)()),
        None => true,
    })
}

fn kind_has_members(kind: ResolverKind) -> bool {
    inventory::iter::<ResolverRegistration>()
        .any(|reg| reg.kind == kind && is_member_active(reg))
}

fn build_members(container: &Container, kind: ResolverKind) -> Vec<Box<dyn ResolverObject>> {
    inventory::iter::<ResolverRegistration>()
        .filter(|reg| reg.kind == kind && is_member_active(reg))
        .map(|reg| (reg.build)(container))
        .collect()
}

/// Merge the fields of every registered object of `kind` into one root object
/// named `type_name`. The member object types are registered as a side effect
/// of `create_fake_output_type` but go unreferenced, so async-graphql's
/// `remove_unused_types` drops them — only the merged root remains in the SDL.
fn merge_type_info<T: OutputType>(
    registry: &mut Registry,
    kind: ResolverKind,
    type_name: &str,
) -> String {
    registry.create_output_type::<T, _>(MetaTypeId::Object, |registry| {
        let mut fields = IndexMap::new();
        for reg in inventory::iter::<ResolverRegistration>() {
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
    ($name:ident, $kind:expr, $type_name:literal) => {
        // Runtime-merged root, internal to the crate; only `build_schema` and
        // `GraphqlModule` name it.
        pub(crate) struct $name {
            members: Vec<Box<dyn ResolverObject>>,
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

discovered_root!(DiscoveredQuery, ResolverKind::Query, "Query");
discovered_root!(DiscoveredMutation, ResolverKind::Mutation, "Mutation");

/// Build the discovered schema. Queries and mutations come from the registry;
/// subscriptions are not yet supported (`SubscriptionType` is a separate trait
/// — tracked as follow-up). The container is attached as schema data and used
/// to construct each resolver via its `from_container`.
///
/// Module-gating: reads [`ReachableProviders`] from the container and installs
/// it in the [`REACHABLE`] thread-local for the duration of `Schema::build`,
/// so the static `OutputType` methods async-graphql invokes filter the
/// resolver inventory to the modules an app actually imports. Cleared on the
/// way out so a subsequent build on the same thread (a test booting multiple
/// apps) starts clean.
pub(crate) fn build_schema(
    container: Container,
) -> Schema<DiscoveredQuery, DiscoveredMutation, EmptySubscription> {
    let reachable = container
        .get::<ReachableProviders>()
        .map(|p| Arc::new(p.0.clone()));
    REACHABLE.with(|cell| *cell.borrow_mut() = reachable);
    let schema = Schema::build(
        DiscoveredQuery::from_registry(&container),
        DiscoveredMutation::from_registry(&container),
        EmptySubscription,
    )
    .data(container.clone())
    .extension(crate::loader::LoaderExtensionFactory::new(container))
    .finish();
    REACHABLE.with(|cell| *cell.borrow_mut() = None);
    schema
}

/// Render the composed schema as SDL for a committed `schema.graphql`.
///
/// Types, fields, arguments, and enum values are sorted so the output is
/// deterministic: the resolver registry's link-time iteration order (which is
/// not stable across builds) cannot leak into the file and churn its diff.
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
