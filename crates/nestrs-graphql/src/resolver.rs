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

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::Field;
use async_graphql::registry::{MetaType, MetaTypeId, Registry};
use async_graphql::{
    CacheControl, ContainerType, Context, ContextSelectionSet, EmptySubscription, ObjectType,
    OutputType, Positioned, SDLExportOptions, Schema, ServerResult, Value,
};
use nestrs_core::Container;

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
/// the container at schema-build time.
#[doc(hidden)]
pub struct ResolverRegistration {
    pub kind: ResolverKind,
    pub type_info: fn(&mut Registry) -> MetaType,
    pub build: fn(&Container) -> Box<dyn ResolverObject>,
}

inventory::collect!(ResolverRegistration);

fn kind_has_members(kind: ResolverKind) -> bool {
    inventory::iter::<ResolverRegistration>().any(|reg| reg.kind == kind)
}

fn build_members(container: &Container, kind: ResolverKind) -> Vec<Box<dyn ResolverObject>> {
    inventory::iter::<ResolverRegistration>()
        .filter(|reg| reg.kind == kind)
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
            if reg.kind != kind {
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
pub(crate) fn build_schema(
    container: Container,
) -> Schema<DiscoveredQuery, DiscoveredMutation, EmptySubscription> {
    Schema::build(
        DiscoveredQuery::from_registry(&container),
        DiscoveredMutation::from_registry(&container),
        EmptySubscription,
    )
    .data(container.clone())
    .extension(crate::loader::LoaderExtensionFactory::new(container))
    .finish()
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
