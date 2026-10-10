//! Runtime schema composition from a link-time resolver registry.
//!
//! `#[operations]` submits generated `#[Object]` / `#[Subscription]` structs to
//! the [`inventory`] registry; the roots [`DiscoveredQuery`] /
//! [`DiscoveredMutation`] / [`DiscoveredSubscription`] merge their fields at
//! build time — the runtime analog of async-graphql's `MergedObject`.
//!
//! `create_type_info` / `is_empty` are static (no container access), so the
//! module-gating reachable set lives in a thread-local installed by
//! [`build_schema`] for the build's duration.

use nest_rs_http::DetachedWork;
use std::any::TypeId;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::futures_util::stream::Stream;
use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::Field;
use async_graphql::registry::{MetaType, MetaTypeId, Registry};
use async_graphql::{
    CacheControl, ContainerType, Context, ContextSelectionSet, ObjectType, OutputType, Positioned,
    Response, SDLExportOptions, Schema, ServerResult, SubscriptionType, Value,
};
use nest_rs_core::{Container, ReachableProviders};

use crate::config::GraphqlConfig;

/// Which root a resolver's methods contribute to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphqlResolverKind {
    /// The `Query` root.
    Query,
    /// The `Mutation` root.
    Mutation,
    /// The `Subscription` root.
    Subscription,
}

impl GraphqlResolverKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
            Self::Subscription => "subscription",
        }
    }
}

/// Object-safe view of a code-first resolver (`ContainerType` is not), blanket
/// implemented for every `#[Object]` type.
pub trait GraphqlResolverObject: Send + Sync {
    /// Resolve one field of this root, as `ContainerType::resolve_field` does.
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>>;

    /// Resolve one federation **reference** — `{__typename, <key fields>}` —
    /// against this member's `#[entity]` resolvers. A root forwarding only
    /// `resolve_field` answers *Entity not found* to every reference.
    fn find_entity<'a>(
        &'a self,
        ctx: &'a Context<'a>,
        params: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>>;
}

impl<T: ContainerType + Send + Sync> GraphqlResolverObject for T {
    fn resolve_field<'a>(
        &'a self,
        ctx: &'a Context<'a>,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>> {
        Box::pin(ContainerType::resolve_field(self, ctx))
    }

    fn find_entity<'a>(
        &'a self,
        ctx: &'a Context<'a>,
        params: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ServerResult<Option<Value>>> + Send + 'a>> {
        Box::pin(ContainerType::find_entity(self, ctx, params))
    }
}

/// Object-safe view of a code-first **subscription** resolver
/// ([`SubscriptionType`], answering with a stream), blanket implemented for
/// every `#[Subscription]` type.
pub trait GraphqlSubscriptionObject: Send + Sync {
    /// The stream one subscription field answers, as
    /// `SubscriptionType::create_field_stream` does.
    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>>;
}

impl<T: SubscriptionType> GraphqlSubscriptionObject for T {
    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>> {
        SubscriptionType::create_field_stream(self, ctx)
    }
}

/// What a registration builds: the two root shapes async-graphql distinguishes.
pub enum GraphqlRootMember {
    /// A `#[query]` / `#[mutation]` root object.
    Object(Box<dyn GraphqlResolverObject>),
    /// A `#[subscription]` root.
    Subscription(Box<dyn GraphqlSubscriptionObject>),
}

/// One `#[resolver]` struct linked into the binary, submitted by the struct
/// half of the pair, so the unreachable-resolver boot warning sees even one
/// carrying no operations.
pub struct ResolverDescriptor {
    /// `TypeId` of the `#[resolver]` struct, matched against reachable modules'
    /// `providers = [...]` to decide whether the resolver is under the contract.
    pub resolver: fn() -> TypeId,
    /// The resolver type's name, surfaced in the unreachable-resolver boot warn.
    pub name: &'static str,
}

inventory::collect!(ResolverDescriptor);

/// Linked `#[resolver]` structs in no module reachable from the app's root,
/// read off [`ReachableProviders`]; without that set nothing is reported.
pub(crate) fn unreachable_resolvers(container: &Container) -> Vec<&'static str> {
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return Vec::new();
    };
    inventory::iter::<ResolverDescriptor>()
        .filter(|d| !reachable.0.contains(&(d.resolver)()))
        .map(|d| d.name)
        .collect()
}

/// One generated resolver object, submitted by `#[operations]` and
/// module-gated by `resolver_type_id`.
pub struct GraphqlResolverRegistration {
    /// The root this object contributes to.
    pub kind: GraphqlResolverKind,
    /// The resolver struct name (`UsersResolver`), logged beside each mounted
    /// operation at boot.
    pub resolver_name: &'static str,
    /// The resolver struct's type, read against the reachable providers.
    pub resolver_type_id: fn() -> TypeId,
    /// One `(method, resolved GraphQL type name)` per `#[entity]` this
    /// registration declares — empty for a root that declares none.
    ///
    /// Read back at boot against what the registry keyed: `Registry::add_keys`
    /// returns silently when the type is neither an object nor an interface.
    pub entities: fn() -> Vec<(&'static str, String)>,
    /// Register the object's type, as async-graphql's `OutputType` does.
    pub type_info: fn(&mut Registry) -> MetaType,
    /// Build the root object from the assembled container.
    pub build: fn(&Container) -> GraphqlRootMember,
}

inventory::collect!(GraphqlResolverRegistration);

thread_local! {
    // `None` => no gating: a bare `Schema::build` includes every linked resolver.
    static REACHABLE: RefCell<Option<Arc<HashSet<TypeId>>>> = const { RefCell::new(None) };
}

fn is_member_active(reg: &GraphqlResolverRegistration) -> bool {
    REACHABLE.with(|cell| match &*cell.borrow() {
        Some(set) => set.contains(&(reg.resolver_type_id)()),
        None => true,
    })
}

fn kind_has_members(kind: GraphqlResolverKind) -> bool {
    inventory::iter::<GraphqlResolverRegistration>()
        .any(|reg| reg.kind == kind && is_member_active(reg))
}

fn build_members(
    container: &Container,
    kind: GraphqlResolverKind,
) -> Vec<Box<dyn GraphqlResolverObject>> {
    inventory::iter::<GraphqlResolverRegistration>()
        .filter(|reg| reg.kind == kind && is_member_active(reg))
        .filter_map(|reg| match (reg.build)(container) {
            GraphqlRootMember::Object(object) => Some(object),
            GraphqlRootMember::Subscription(_) => None,
        })
        .collect()
}

fn build_subscription_members(container: &Container) -> Vec<Box<dyn GraphqlSubscriptionObject>> {
    inventory::iter::<GraphqlResolverRegistration>()
        .filter(|reg| reg.kind == GraphqlResolverKind::Subscription && is_member_active(reg))
        .filter_map(|reg| match (reg.build)(container) {
            GraphqlRootMember::Subscription(root) => Some(root),
            GraphqlRootMember::Object(_) => None,
        })
        .collect()
}

/// The merged field map for one root, logging each mounted operation.
fn merge_fields(
    registry: &mut Registry,
    kind: GraphqlResolverKind,
) -> IndexMap<String, async_graphql::registry::MetaField> {
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
            for field_name in member_fields.keys() {
                tracing::info!(
                    target: nest_rs_http::target::ROUTES,
                    resolver = reg.resolver_name,
                    kind = kind.as_str(),
                    field = field_name.as_str(),
                    "mounted operation",
                );
            }
            fields.extend(member_fields);
        }
    }
    fields
}

/// The merged root's `MetaType`; `is_subscription` is a parameter so there is
/// one exhaustive literal for the version canary below to pin.
fn root_meta_type<T>(
    type_name: &str,
    fields: IndexMap<String, async_graphql::registry::MetaField>,
    is_subscription: bool,
) -> MetaType {
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
        is_subscription,
        rust_typename: Some(std::any::type_name::<T>()),
        directive_invocations: Default::default(),
        requires_scopes: Default::default(),
    }
}

/// Merge fields of every registered object of `kind` into one root object.
/// Member object types register as a side effect but go unreferenced, so
/// `remove_unused_types` drops them from the SDL.
fn merge_type_info<T: OutputType>(
    registry: &mut Registry,
    kind: GraphqlResolverKind,
    type_name: &str,
) -> String {
    registry.create_output_type::<T, _>(MetaTypeId::Object, |registry| {
        let fields = merge_fields(registry, kind);
        root_meta_type::<T>(type_name, fields, false)
    })
}

/// [`merge_type_info`] for the subscription root, which is not an `OutputType`.
fn merge_subscription_type_info<T: SubscriptionType>(
    registry: &mut Registry,
    type_name: &str,
) -> String {
    registry.create_subscription_type::<T, _>(|registry| {
        let fields = merge_fields(registry, GraphqlResolverKind::Subscription);
        root_meta_type::<T>(type_name, fields, true)
    })
}

/// Boot check: two reachable resolvers may not claim one operation name, nor
/// one `@key` shape.
///
/// [`merge_type_info`] keeps the **last** registration's metadata while
/// `DiscoveredQuery::resolve_field` answers from the **first**, so the SDL would
/// document one body while another — with its own `#[authorize]` — runs.
///
/// Field names come from `type_info`, the source `merge_type_info` reads, so
/// async-graphql's `rename_rule` is not reimplemented. Returns the first
/// reachable resolver that declared an `#[entity]`.
pub(crate) fn check_operations(container: &Container) -> Result<Option<&'static str>, String> {
    let reachable = container.get::<ReachableProviders>().map(|p| p.0.clone());
    // A scratch registry: registering is a side effect the served schema must not see.
    let mut scratch = Registry::default();
    let mut claimed: HashMap<(GraphqlResolverKind, String), &'static str> = HashMap::new();
    let mut clashes: Vec<String> = Vec::new();
    // An `#[entity]` claims a type and a key shape through `add_keys`, never a field.
    let mut keyed: HashMap<(String, String), &'static str> = HashMap::new();
    // Per type, so the overlap check can compare shapes pairwise.
    let mut claims: HashMap<String, Vec<(String, &'static str)>> = HashMap::new();
    // `#[entity]` methods the registry keyed nothing for.
    let mut unkeyed: Vec<String> = Vec::new();
    let mut first_entity: Option<&'static str> = None;

    for reg in inventory::iter::<GraphqlResolverRegistration>() {
        if let Some(set) = reachable.as_ref()
            && !set.contains(&(reg.resolver_type_id)())
        {
            continue;
        }
        // Diffed rather than counted: a cursor would rely on async-graphql
        // appending keys, and would pass a clash if it ever stopped.
        let before = keyed_types(&scratch);
        let type_info = (reg.type_info)(&mut scratch);
        let after = keyed_types(&scratch);
        // `add_keys` is a silent no-op on anything not an object or interface.
        for (method, resolved) in (reg.entities)() {
            first_entity.get_or_insert(reg.resolver_name);
            if !after.contains_key(&resolved) {
                unkeyed.push(format!(
                    "`{}::{method}` resolves {resolved:?}",
                    reg.resolver_name,
                ));
            }
        }
        for (name, keys) in after.iter() {
            let already = before.get(name);
            for (index, key) in keys.iter().enumerate() {
                if already.is_some_and(|seen: &Vec<String>| seen.get(index) == Some(key)) {
                    continue;
                }
                claims
                    .entry(name.clone())
                    .or_default()
                    .push((key.clone(), reg.resolver_name));
                // The key shape is the identity: disjoint keys on one type both
                // stay reachable; one shape claimed twice is answered by link order.
                match keyed.entry((name.clone(), key.clone())) {
                    Entry::Vacant(slot) => {
                        slot.insert(reg.resolver_name);
                    }
                    // Two `#[entity]` methods in one `impl` register in one pass.
                    Entry::Occupied(first) => clashes.push(format!(
                        "entity {name:?} keyed by {key:?} ({} and {})",
                        first.get(),
                        reg.resolver_name,
                    )),
                }
            }
        }
        let MetaType::Object { fields, .. } = type_info else {
            continue;
        };
        for field in fields.keys() {
            match claimed.entry((reg.kind, field.clone())) {
                Entry::Vacant(slot) => {
                    slot.insert(reg.resolver_name);
                }
                Entry::Occupied(first) => clashes.push(format!(
                    "{} {:?} ({} and {})",
                    reg.kind.as_str(),
                    field,
                    first.get(),
                    reg.resolver_name,
                )),
            }
        }
    }

    if !unkeyed.is_empty() {
        unkeyed.sort();
        return Err(format!(
            "`#[entity]` that keys nothing: {} — async-graphql adds a `@key` to an \
             **object or interface** type and returns silently for anything else, \
             so a list, a scalar or a union resolves to a type name the registry \
             never keys. The method then registers no key, the type never joins \
             `_entities`, and the resolver is unreachable code the schema does not \
             mention — which also disarms the `federation` refusal, since a schema \
             with no key looks like a schema with no entity. Return the object \
             itself: `Result<T>`, or `Result<Option<T>>` for a reference that may \
             resolve to nothing.",
            unkeyed.join(", "),
        ));
    }

    if !clashes.is_empty() {
        clashes.sort();
        return Err(format!(
            "duplicate GraphQL operation name: {} — an operation is addressed by \
             bare name within a schema, so the SDL would publish one resolver's \
             signature while the other resolver's body ran. Rename one of them. \
             An `entity` clash is the same defect addressed by `@key` instead of by \
             name: two `#[entity]` resolvers for one type, of which the router \
             reaches whichever linked first — including its access posture.",
            clashes.join(", "),
        ));
    }

    let mut overlaps = overlapping_claims(&claims);
    if !overlaps.is_empty() {
        overlaps.sort();
        return Err(format!(
            "overlapping `@key` shapes on one GraphQL entity: {} — a reference \
             carrying the wider shape's fields satisfies the narrower claim as \
             well, so one of the two bodies is unreachable and the access posture \
             that runs is the other one's. Nothing orders the two by anything you \
             declared: across resolvers `_entities` answers from whichever linked \
             first, and within one resolver async-graphql sorts its matchers by \
             *argument count* — not key arity — so a key with a non-key argument \
             beside it outranks a longer key without one. Keep the shapes \
             disjoint: `@key(fields: \"id\")` beside `@key(fields: \"slug\")` \
             shares no field, so a reference selects the body rather than the \
             link order.",
            overlaps.join(", "),
        ));
    }

    Ok(first_entity)
}

/// The claim pairs on one type whose key shapes are neither equal nor
/// **disjoint**, including on one resolver: async-graphql's `object.rs` sorts
/// entity matchers by *argument count*, not key arity, so nothing a developer
/// declared orders the two bodies. Narrower than Apollo on purpose.
///
/// Compared as field sets, so a permutation (`"id tenant"` / `"tenant id"`)
/// lands here; equal strings are the exact-shape check's.
fn overlapping_claims(claims: &HashMap<String, Vec<(String, &'static str)>>) -> Vec<String> {
    let mut overlaps = Vec::new();
    for (name, declared) in claims {
        for (index, (key, owner)) in declared.iter().enumerate() {
            for (other_key, other_owner) in &declared[index + 1..] {
                if key == other_key {
                    continue;
                }
                let (fields, other_fields) = (key_fields(key), key_fields(other_key));
                if !fields.is_subset(&other_fields) && !other_fields.is_subset(&fields) {
                    continue;
                }
                let (first, first_owner, second, second_owner) =
                    if fields.len() <= other_fields.len() {
                        (key, owner, other_key, other_owner)
                    } else {
                        (other_key, other_owner, key, owner)
                    };
                overlaps.push(format!(
                    "entity {name:?} keyed by {first:?} ({first_owner}) and by \
                     {second:?} ({second_owner})",
                ));
            }
        }
    }
    overlaps
}

/// The **top-level** field names one `@key(fields: …)` selects, depth-aware:
/// `"id organization { id }"` selects `id` and `organization`.
fn key_fields(key: &str) -> BTreeSet<&str> {
    let mut fields = BTreeSet::new();
    let mut depth = 0usize;
    let mut rest = key;
    while let Some(index) = rest.find(|c: char| !c.is_whitespace()) {
        rest = &rest[index..];
        match rest.as_bytes()[0] {
            b'{' => {
                depth += 1;
                rest = &rest[1..];
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                rest = &rest[1..];
            }
            _ => {
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '{' || c == '}')
                    .unwrap_or(rest.len());
                if depth == 0 {
                    fields.insert(&rest[..end]);
                }
                rest = &rest[end..];
            }
        }
    }
    fields
}

/// Every type in `registry` carrying federation keys, and the key shapes it
/// carries, in registration order.
fn keyed_types(registry: &Registry) -> HashMap<String, Vec<String>> {
    registry
        .types
        .iter()
        .filter_map(|(name, ty)| match ty {
            MetaType::Object {
                keys: Some(keys), ..
            }
            | MetaType::Interface {
                keys: Some(keys), ..
            } if !keys.is_empty() => Some((name.clone(), keys.clone())),
            _ => None,
        })
        .collect()
}

/// Compile-time canary for the pinned async-graphql registry API: the literal
/// in `root_meta_type` breaks on a removed field, this destructure (no `..`) on
/// an added one. After a bump, update both, then review the SDL snapshot diff.
const _: () = {
    #[expect(dead_code, reason = "a compile-time canary, never called")]
    fn metatype_object_field_canary(ty: MetaType) {
        if let MetaType::Object {
            name: _,
            description: _,
            fields: _,
            cache_control: _,
            extends: _,
            shareable: _,
            resolvable: _,
            keys: _,
            visible: _,
            inaccessible: _,
            interface_object: _,
            tags: _,
            is_subscription: _,
            rust_typename: _,
            directive_invocations: _,
            requires_scopes: _,
        } = ty
        {}
    }
};

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

            /// First member that answers: `check_operations` leaves only
            /// disjoint key shapes, so at most one can match.
            async fn find_entity(
                &self,
                ctx: &Context<'_>,
                params: &Value,
            ) -> ServerResult<Option<Value>> {
                for member in &self.members {
                    if let Some(value) = member.find_entity(ctx, params).await? {
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
discovered_root!(
    DiscoveredMutation,
    GraphqlResolverKind::Mutation,
    "Mutation"
);

/// The discovered `Subscription` root — [`discovered_root!`]'s streaming
/// sibling; [`SubscriptionType`] shares no method with `ContainerType`.
pub(crate) struct DiscoveredSubscription {
    members: Vec<Box<dyn GraphqlSubscriptionObject>>,
}

impl DiscoveredSubscription {
    fn from_registry(container: &Container) -> Self {
        Self {
            members: build_subscription_members(container),
        }
    }
}

impl SubscriptionType for DiscoveredSubscription {
    fn type_name() -> Cow<'static, str> {
        Cow::Borrowed("Subscription")
    }

    fn create_type_info(registry: &mut Registry) -> String {
        merge_subscription_type_info::<Self>(registry, "Subscription")
    }

    fn is_empty() -> bool {
        !kind_has_members(GraphqlResolverKind::Subscription)
    }

    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>> {
        self.members
            .iter()
            .find_map(|member| member.create_field_stream(ctx))
    }
}

/// The schema this crate composes from three discovered roots.
pub(crate) type DiscoveredSchema =
    Schema<DiscoveredQuery, DiscoveredMutation, DiscoveredSubscription>;

/// Build the discovered schema.
///
/// Installs [`ReachableProviders`] in [`REACHABLE`] for the duration of
/// `Schema::build`; the drop guard restores the previous value even on panic,
/// so one build's set never leaks into another's on the same thread.
pub(crate) fn build_schema(
    container: Container,
    config: &GraphqlConfig,
    batches: &DetachedWork,
) -> DiscoveredSchema {
    let reachable = container
        .get::<ReachableProviders>()
        .map(|p| Arc::new(p.0.clone()));
    let _reset = ReachableResetGuard::set(reachable);
    let mut builder = Schema::build(
        DiscoveredQuery::from_registry(&container),
        DiscoveredMutation::from_registry(&container),
        DiscoveredSubscription::from_registry(&container),
    )
    .data(container.clone())
    .extension(crate::loader::LoaderExtensionFactory::new(
        container.clone(),
        batches.clone(),
    ));
    // Whatever `config.federation` says: async-graphql serves those fields from
    // the keys alone.
    if let Some(gate) =
        crate::federation::FederationExtensionFactory::from_container(&container, config)
    {
        builder = builder.extension(gate);
    }
    if let Some(d) = config.max_depth {
        builder = builder.limit_depth(d);
    }
    if let Some(c) = config.max_complexity {
        builder = builder.limit_complexity(c);
    }
    if config.disable_introspection {
        builder = builder.disable_introspection();
    }
    if config.federation {
        // An `#[entity]`'s `add_keys` creates the fields on its own; this covers a
        // subgraph with no key yet.
        builder = builder.enable_federation();
    }
    builder.finish()
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

/// Render the composed schema as SDL, sorted: link-time iteration order is not
/// stable. A subgraph exports the Apollo **subgraph form** (`@key` present,
/// `_service` / `_entities` stripped).
pub(crate) fn render_sdl(schema: &DiscoveredSchema, config: &GraphqlConfig) -> String {
    let options = SDLExportOptions::new()
        .sorted_fields()
        .sorted_arguments()
        .sorted_enum_items();
    let options = if config.federation {
        options.federation()
    } else {
        options
    };
    schema.sdl_with_options(options)
}
