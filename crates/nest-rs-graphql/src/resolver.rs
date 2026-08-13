//! Runtime schema composition from a link-time resolver registry.
//!
//! `#[operations]` splits its `#[query]`/`#[mutation]`/`#[subscription]`
//! methods into generated `#[Object]` / `#[Subscription]` structs and submits
//! each to the [`inventory`] registry. The roots [`DiscoveredQuery`] /
//! [`DiscoveredMutation`] / [`DiscoveredSubscription`] are static types whose
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
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphqlResolverKind {
    Query,
    Mutation,
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

    /// Resolve one federation **reference** — `{__typename, <key fields>}` —
    /// against this member's `#[entity]` resolvers.
    ///
    /// A second entry point rather than a case of the first, because the router
    /// does not name a field: it hands `_entities` a representation, and the
    /// derive's `find_entity` is what matches it against the keys this member
    /// declared. A root that forwards only `resolve_field` therefore answers
    /// *Entity not found* to every reference, however many entity resolvers the
    /// app wrote — which is how a merged root loses a whole operation role
    /// without losing a field.
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

/// Object-safe view of a code-first **subscription** resolver — the streaming
/// counterpart of [`GraphqlResolverObject`].
///
/// It cannot reuse that trait: a subscription root implements
/// [`SubscriptionType`], not `ContainerType`, and answers a selected field with
/// a *stream* of responses rather than one value. Same shape otherwise — the
/// static methods (`type_name` / `create_type_info`) are what force the shim,
/// and the blanket impl covers every `#[Subscription]` type.
#[doc(hidden)]
pub trait GraphqlSubscriptionObject: Send + Sync {
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
///
/// One registry rather than two, deliberately: module-gating, the boot-time
/// duplicate-name check and the mounted-operation log all walk *every*
/// contribution, and a second `inventory::collect!` would have to be threaded
/// through each of them — three places for a subscription to be forgotten in.
/// The kind already discriminates; this only carries the built value.
#[doc(hidden)]
pub enum GraphqlRootMember {
    /// A `#[query]` / `#[mutation]` root object.
    Object(Box<dyn GraphqlResolverObject>),
    /// A `#[subscription]` root.
    Subscription(Box<dyn GraphqlSubscriptionObject>),
}

/// One generated resolver object, submitted by `#[operations]`.
/// `resolver_type_id` keys the entry against [`ReachableProviders`] for
/// module-gating.
#[doc(hidden)]
pub struct GraphqlResolverRegistration {
    pub kind: GraphqlResolverKind,
    /// The resolver struct name (`UsersResolver`) — logged as a structured
    /// field beside each mounted operation at boot, mirroring `#[routes]`.
    pub resolver_name: &'static str,
    pub resolver_type_id: fn() -> TypeId,
    /// One `(method, resolved GraphQL type name)` per `#[entity]` this
    /// registration declares — empty for a root that declares none.
    ///
    /// The *declaration*, read back at boot against what the registry actually
    /// keyed. async-graphql's `Registry::add_keys` returns silently when the
    /// named type is neither an object nor an interface, so a `#[entity]`
    /// resolving to a `Vec<T>` or a scalar compiles, boots, publishes no `@key`,
    /// and is unreachable — with `_entities` never created, which also disarms
    /// the `federation = false` refusal that reads the same pass.
    pub entities: fn() -> Vec<(&'static str, String)>,
    pub type_info: fn(&mut Registry) -> MetaType,
    pub build: fn(&Container) -> GraphqlRootMember,
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

/// The merged field map for one root, plus the boot-time "mounted operation"
/// log each contribution earns. Shared by the object roots and the subscription
/// root — the merge is the same walk either way; only the `MetaType` wrapper
/// and the registry entry point differ.
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
                    target: "nest_rs::routes",
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

/// The merged root's `MetaType`. `is_subscription` is the one field that
/// differs between the two roots, and async-graphql reads it when rendering the
/// SDL — so it is a parameter rather than a second literal that could drift
/// from the exhaustive one the version canary below pins.
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
/// Member object types register as a side effect of `create_fake_output_type`
/// but go unreferenced, so `remove_unused_types` drops them — only the merged
/// root remains in the SDL.
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

/// [`merge_type_info`] for the subscription root. Registered through
/// `create_subscription_type` — a `#[Subscription]` type is not an `OutputType`,
/// so the object entry point cannot register it.
fn merge_subscription_type_info<T: SubscriptionType>(
    registry: &mut Registry,
    type_name: &str,
) -> String {
    registry.create_subscription_type::<T, _>(|registry| {
        let fields = merge_fields(registry, GraphqlResolverKind::Subscription);
        root_meta_type::<T>(type_name, fields, true)
    })
}

/// Boot check: two reachable resolvers may not claim one operation name.
///
/// Merging is what a transport does — several providers contribute to one
/// mount — and it introduces exactly one new failure mode: two contributions
/// claiming the same addressable name. HTTP and MCP already fail the boot
/// naming both owners; this is GraphQL's, and until it existed the two halves
/// of the merge did not even agree on a winner.
///
/// [`merge_type_info`] folds member fields with `IndexMap::extend`, so the
/// **last** registration's metadata lands in the schema — while
/// `DiscoveredQuery::resolve_field` returns from the **first** member that
/// answers. A client therefore reads one resolver's signature from the SDL and
/// reaches another resolver's body, with the arguments it was told to send
/// dropped. Both orders follow `inventory::iter`, which is link order.
///
/// That is a security defect, not only a confusing one: `#[authorize]` expands
/// *inside* the operation's body, so the posture that runs belongs to whichever
/// body won the dispatch, not to the operation the schema documents.
///
/// Field names come from `type_info` — the same source `merge_type_info` reads
/// — rather than from names the macro could have submitted, because the wire
/// name is async-graphql's `rename_rule` applied to the method, and a second
/// implementation of that rule here would be free to drift from the one that
/// actually builds the schema.
/// Returns the first reachable resolver that declared an `#[entity]`, which is
/// the other question this one pass answers. Building the whole meta-type
/// registry is the expensive half of a GraphQL boot, and asking "does anything
/// declare a key?" is reading the map this already builds — a second pass for it
/// would also be a second copy of what counts as an entity and of the
/// reachability filter, free to disagree with this one.
pub(crate) fn check_operations(container: &Container) -> Result<Option<&'static str>, String> {
    let reachable = container.get::<ReachableProviders>().map(|p| p.0.clone());
    // A scratch registry, never the schema's: `create_fake_output_type`
    // registers as a side effect, and this pass must leave no trace on the
    // registry the served schema is built from.
    let mut scratch = Registry::default();
    let mut claimed: HashMap<(GraphqlResolverKind, String), &'static str> = HashMap::new();
    let mut clashes: Vec<String> = Vec::new();
    // An `#[entity]` claims a **type and a key shape**, not a field name, so the
    // loop below cannot see it: async-graphql's derive routes an entity method
    // into `add_keys` and never into the object's fields. It lands in the
    // scratch registry all the same, which is what this reads.
    let mut keyed: HashMap<(String, String), &'static str> = HashMap::new();
    // Every claim, kept per type so the *overlap* check below can compare shapes
    // pairwise. `keyed` answers "is this exact shape taken?" and cannot: two
    // shapes that clash are two different map entries.
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
        // The keys this registration is about to add, read by diffing the
        // registry against itself.
        //
        // **A key count would be cheaper and is refused.** Keys append today, so
        // "everything past the cursor is new" holds — but it holds *because of*
        // an upstream invariant this code cannot check, and the two forms fail in
        // opposite directions if async-graphql ever stops appending: comparing
        // the shapes attributes a replaced key to the registration that replaced
        // it, at worst reporting a clash that is not one, which is a boot error
        // someone reads; a cursor skips it, and a later claim on that shape finds
        // no first claimant and passes. This check is what stops two `#[entity]`
        // resolvers for one type — of which the router reaches whichever linked
        // first, *including its access posture* — so it is loose only where the
        // cost is a loud failure. A `HashMap` rebuilt per registration at boot is
        // what that costs.
        let before = keyed_types(&scratch);
        let type_info = (reg.type_info)(&mut scratch);
        let after = keyed_types(&scratch);
        // Every `#[entity]` this registration declared, against what the
        // registry actually keyed. `add_keys` is a silent no-op on anything that
        // is not an object or an interface, so this is the only place the two can
        // be compared — a resolver may not simply be *asked* whether its method
        // is an entity, since the answer async-graphql gives is the registry's.
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
                // **The key shape is the identity, not the type.** Apollo lets a
                // type carry several `@key`s, and async-graphql's `find_entity`
                // matches a reference against the shape — so two resolvers
                // keying one type by **disjoint** fields both stay reachable and
                // are not a duplicate. Two claims on the same shape are: only
                // one body can answer, and which one is link order. Shapes that
                // are neither equal nor disjoint are [`overlapping_claims`],
                // wherever they were declared.
                match keyed.entry((name.clone(), key.clone())) {
                    Entry::Vacant(slot) => {
                        slot.insert(reg.resolver_name);
                    }
                    // A resolver can clash with **itself**: two `#[entity]`
                    // methods for one type in one `impl` register inside a
                    // single pass, so a check that diffed per registration
                    // could never see them — which is the arrangement the whole
                    // branch exists for, written the way it is easiest to write.
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
/// **disjoint**.
///
/// **Wherever they are declared, including on one resolver**, and the exemption
/// that used to be here is why. It rested on "async-graphql orders a single
/// `#[Object]`'s entity matchers by key arity, so the more specific one wins" —
/// which is not what upstream does. `object.rs` pushes `(args.len(), code)` and
/// sorts on *that*: a method with a non-key argument beside its key counts
/// higher than a method with more key fields and none, so the **narrow** matcher
/// can be tried first and the wide body — with its own `#[authorize]` — becomes
/// unreachable. Equal arity is worse still: the sort is stable, so
/// `@key(fields: "id tenant")` beside `@key(fields: "tenant id")` is decided by
/// declaration order and one of the two is dead code the SDL still advertises.
///
/// So the rule is the same at both scopes: **disjoint, or one claim**. That is
/// narrower than Apollo, which allows a type several overlapping `@key`s — and
/// the narrowing is the honest position, because what a router picks then
/// decides which access posture answers, and neither this framework nor
/// async-graphql orders those two bodies by anything a developer declared.
///
/// Equal *strings* are excluded because the exact-shape check above already
/// names them; a permutation is not equal as a string and lands here, which is
/// why the comparison is on the field **set**.
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
                // The narrower shape first: it is the one that matches every
                // reference the other does, which is what makes the pair a clash.
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

/// The **top-level** field names one `@key(fields: …)` selects.
///
/// Depth-aware rather than a whitespace split, because Apollo's key syntax
/// nests: `"id organization { id }"` selects `id` and `organization`, and a
/// split on whitespace reads the inner `id` as a second top-level field — which
/// made `@key(fields: "id")` look like a subset of `@key(fields: "org { id }")`
/// and refused a pair that shares no field at all. What `find_entity` matches on
/// is the presence of the top-level names, so those are what decides overlap.
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
///
/// The shapes, not a count: the shape is what says whether two claims collide,
/// several `@key`s per type being Apollo's. That a second `#[entity]` for one
/// type **appends** to the same vector rather than replacing it is what lets a
/// cursor say which entries a given registration added. The doubled
/// `@key(fields: "id") @key(fields: "id")` a same-shape duplicate leaves in the
/// SDL is the only other trace of it.
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

/// Compile-time canary for the pinned async-graphql registry API.
///
/// `merge_type_info` above constructs a `MetaType::Object { .. }` with an
/// **exhaustive** field list; that literal already breaks the build if a field
/// is *removed* or *renamed* upstream. This destructure closes the other half:
/// it matches every field with **no `..` rest pattern**, so an *added* field
/// also fails compilation right here rather than being silently ignored.
///
/// The workspace pins `async-graphql = "=7.2.1"` precisely so this stays in
/// lockstep. If this block stops compiling after a bump, the registry shape
/// changed — do all of:
///   1. update the `MetaType::Object { .. }` literal in `merge_type_info`,
///   2. mirror the new/removed field in the destructure below,
///   3. re-pin `async-graphql`/`async-graphql-poem` in the root `Cargo.toml`,
///   4. run the SDL snapshot test (`tests/integration/sdl_snapshot.rs`) and
///      review the schema diff.
const _: () = {
    #[allow(dead_code)]
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

            /// Same first-member-that-answers rule as `resolve_field`, and the
            /// boot is what keeps it unambiguous: two members whose key shapes
            /// are equal — or merely overlap, one being a subset of the other —
            /// fail `check_operations`, which reads the keys out of its scratch
            /// registry precisely because an entity claims a type rather than a
            /// field name and leaves nothing else to clash. What survives to
            /// here is a set of **disjoint** shapes, where at most one member
            /// can match any given reference and the order is therefore moot.
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
/// sibling, written out rather than folded into the macro because
/// [`SubscriptionType`] shares no method with `OutputType` + `ContainerType`.
///
/// `is_empty` is what keeps a schema with no `#[subscription]` anywhere free of
/// an empty `Subscription` type: async-graphql skips the root entirely, exactly
/// as it does for `EmptySubscription`.
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
        // First member that claims the field answers — the same dispatch rule
        // the object roots use, and the same reason two members may not claim
        // one name (`check_operations`).
        self.members
            .iter()
            .find_map(|member| member.create_field_stream(ctx))
    }
}

/// The schema this crate composes: three discovered roots, no compile-time
/// `MergedObject` tuple anywhere.
pub(crate) type DiscoveredSchema =
    Schema<DiscoveredQuery, DiscoveredMutation, DiscoveredSubscription>;

/// Build the discovered schema.
///
/// Installs [`ReachableProviders`] in [`REACHABLE`] for the duration of
/// `Schema::build`. The drop guard restores the previous value even on panic —
/// a leak would otherwise carry one test's reachable set into another's
/// build on the same thread.
///
/// `config.max_depth` and `config.max_complexity`, when set, become validation
/// limits on every incoming query. Both default to unset to keep the change
/// opt-in; production apps should pin them via `NESTRS_GRAPHQL__MAX_DEPTH` /
/// `__MAX_COMPLEXITY` or the pinned `GraphqlConfig`.
pub(crate) fn build_schema(container: Container, config: &GraphqlConfig) -> DiscoveredSchema {
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
    ));
    // The app-wide chain in front of `_service` / `_entities` — see
    // `crate::federation`. Installed whatever `config.federation` says, because
    // async-graphql serves those fields from the keys alone; the boot refusal in
    // `module.rs` is what makes the flag agree with them, and this is what gates
    // them once it does.
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
        // Declares the federation directives and the two fields a router calls.
        // Not what *creates* them: an `#[entity]` resolver does that on its own
        // (its `add_keys` is what `has_entities()` reads), so this is the switch
        // for a subgraph that has yet to declare its first key.
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

/// Render the composed schema as SDL. Types, fields, arguments, and enum
/// values are sorted: the resolver registry's link-time iteration order is
/// not stable, and would otherwise churn the committed SDL diff.
///
/// A subgraph exports the **subgraph form**: `@key` appears on every federated
/// type, and `_service` / `_entities` are stripped, which is what the Apollo
/// spec asks of an exported subgraph schema. The committed SDL therefore moves
/// in both directions the day an app federates — a fact worth seeing in a diff,
/// which is why the option follows the config rather than being always on.
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
