//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` generates one batching loader per method and submits a
//! [`GraphqlLoaderRegistration`]; [`LoaderSeeds`] rebuilds it per request and
//! seeds it into the GraphQL context for a field to read. A field whose
//! resolver reads one — an exposed relation, a `#[field_resolver]`'s
//! `&DataLoader<L>` parameter — submits a [`GraphqlRelationRegistration`], a
//! root field's parameter a [`GraphqlRootLoaderRegistration`], so a served field
//! whose loader's owner the app does not reach refuses the boot rather than
//! every query selecting it.
//!
//! async-graphql runs every batch on a task of its own, so batches run in the
//! `/graphql` mount's [`DetachedWork`], which the transport stops at the close
//! of its shutdown window.

use std::any::TypeId;
use std::cell::RefCell;
use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, ContextBase, Data, Request};
use futures_util::future::Either;
use nest_rs_core::__private::AnswerContext;
use nest_rs_core::{
    Container, Continuation, Failure, ReachableProviders, RequestContinuation,
    UngatedDiscoveryError, anyhow,
};
use nest_rs_http::DetachedWork;
use tracing::Instrument;

use crate::context::held_by;
use crate::error::{FieldFailure, UnseededLoader};
use crate::operation::field_name;
use crate::resolver::{ServedObjects, import_remedy};

/// One DataLoader registration, module-gated by its owner's reachability:
/// `container.get::<Owner>()` would panic at request time otherwise.
pub struct GraphqlLoaderRegistration {
    /// The `#[dataloader]` owner's type, read against the reachable providers.
    pub owner_type_id: fn() -> TypeId,
    /// Attach the owner's loaders to the request's data.
    pub seed: fn(&Container, &DetachedWork, Request) -> Request,
}

inventory::collect!(GraphqlLoaderRegistration);

/// The provider a `#[dataloader]` loader batches through, which a field
/// reading the loader needs the app to reach — said by a function, since the
/// provider may be private where its loader is not.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is no loader `#[dataloader]` generated, so the boot cannot check that the app reaches what it batches through",
    note = "write the batch as a method of a `#[dataloader]` impl block, and name the loader it generates, `{{Owner}}{{PascalMethod}}`"
)]
pub trait LoaderOwner {
    /// The `#[dataloader]` impl block's own type, and its name as
    /// `std::any::type_name` spells it.
    fn owner() -> (TypeId, &'static str);
}

/// One field whose resolver reads a loader — an exposed relation, a
/// `#[field_resolver]`'s `&DataLoader<L>` parameter — with the object it is
/// served on: the schema build refuses it when that object is served and the
/// loader's owner is not reached.
pub struct GraphqlRelationRegistration {
    /// The object the field is served on, as `std::any::type_name` spells it.
    pub parent: fn() -> &'static str,
    /// The field's GraphQL name.
    pub field: &'static str,
    /// What declares the field — the relation's entity, the field resolver's
    /// parent — as `std::any::type_name` spells it.
    pub entity: fn() -> &'static str,
    /// The loader's owner type, read against the reachable providers, and its
    /// name ([`LoaderOwner::owner`]).
    pub owner: fn() -> (TypeId, &'static str),
    /// Where the field is declared (`module_path!()`), which tells the app's
    /// own declaration from a dependency's.
    pub origin: &'static str,
}

inventory::collect!(GraphqlRelationRegistration);

/// One root field whose method reads a loader — a `#[query]`, `#[mutation]`,
/// `#[subscription]` or `#[entity]`'s `&DataLoader<L>` parameter. The schema
/// serves it whenever the app reaches its resolver, and an `#[entity]` is no
/// field of any object, so it is read against the resolver, never the objects.
pub struct GraphqlRootLoaderRegistration {
    /// The resolver declaring the field, read against the reachable providers,
    /// and its name.
    pub resolver: fn() -> (TypeId, &'static str),
    /// The field as a chain's log line and a refusal name it (`query label`).
    pub field: &'static str,
    /// The loader's owner type, read against the reachable providers, and its
    /// name ([`LoaderOwner::owner`]).
    pub owner: fn() -> (TypeId, &'static str),
    /// Where the field is declared (`module_path!()`), which tells the app's
    /// own declaration from a dependency's.
    pub origin: &'static str,
}

inventory::collect!(GraphqlRootLoaderRegistration);

/// The request's `L` loader for the field `ctx` resolves. Its absence fails the
/// field, never panics: the schema build refuses a field whose loader's owner
/// the app does not reach, so only a schema no `GraphqlModule` built gets here,
/// and the type goes on the operator's line, never to the client.
pub fn seeded_loader<'c, L>(ctx: &Context<'c>) -> Result<&'c DataLoader<L>, FieldFailure>
where
    L: Send + Sync + 'static,
{
    ctx.data_opt::<DataLoader<L>>().ok_or_else(|| {
        FieldFailure::unanswered(Failure::from(UnseededLoader {
            field: field_name(ctx).to_owned(),
            loader: std::any::type_name::<L>(),
        }))
    })
}

/// The loaders a query's or a mutation's own `&DataLoader<L>` parameters read:
/// the request's, or, where the field's unit holds what its request does not —
/// an ability its own guards attached — loaders of the field's own, whose
/// batches judge under it ([`batch_spawner`]).
pub struct UnitLoaders(Option<Data>);

impl UnitLoaders {
    /// The loaders of the root field `ctx` resolves, read inside its unit.
    pub fn of(ctx: &Context<'_>) -> Self {
        if AnswerContext::capture().is_empty() {
            return Self(None);
        }
        Self(LoaderSeeds::seed_own(ctx))
    }

    /// The `L` loader the field reads, failing as [`seeded_loader`] does.
    pub fn get<'a, L>(&'a self, ctx: &Context<'a>) -> Result<&'a DataLoader<L>, FieldFailure>
    where
        L: Send + Sync + 'static,
    {
        match self.0.as_ref().and_then(held_by::<DataLoader<L>>) {
            Some(own) => Ok(own),
            None => seeded_loader(ctx),
        }
    }
}

/// The spawner a request's DataLoader batches run through, filed under
/// `batches`.
///
/// async-graphql calls it on the resolver's task, inside `load_many`, so the
/// capture taken there sees the request: the batch runs under what the request
/// carries into a [`Batch`](Continuation::Batch) — its trace, the caller's
/// ability, the pool rather than its transaction — whichever field triggered
/// it. Built where the request seeds its loaders, before any field runs, it
/// holds the span of the unit whose identity the batch carries — the request's,
/// outside the operation's own — so the export files a batch where its lines
/// do. A field whose unit holds what its request does not — an ability a root
/// field's own guards attached — carries that too, merged over the request's
/// ([`AnswerContext`]): a query's or a mutation's root field reads through
/// loaders of its own there ([`UnitLoaders`], `LoaderSeeds::seed_own`), so no
/// other root field's keys join its batches, and a subscription's operation
/// holds one root field, another refused before it opens (`operation_units`).
pub fn batch_spawner(
    batches: &DetachedWork,
) -> impl Fn(crate::BoxFuture<'static, ()>) + Send + Sync + 'static {
    let batches = batches.clone();
    let span = SEEDING
        .with_borrow(Option::clone)
        .unwrap_or_else(tracing::Span::current);
    move |batch| {
        let batch = AnswerContext::capture().scope(batch);
        let batch = match RequestContinuation::capture(Continuation::Batch) {
            Some(carried) => Either::Left(carried.scope(batch)),
            None => Either::Right(batch),
        };
        let batch = nest_rs_core::panic::contain(batch.instrument(span.clone()));
        let batches = batches.clone();
        tokio::spawn(async move {
            if let Some(Err(unwound)) = batches.run(batch).await {
                nest_rs_core::contained_panic!(
                    target: crate::TARGET,
                    unwound.as_ref(),
                    "a DataLoader batch panicked; the loads it served answer no value",
                );
            }
        });
    }
}

/// The per-request seeding step of one reachable dataloader.
type LoaderSeed = fn(&Container, &DetachedWork, Request) -> Request;

/// The span a request's batches run in, put in its data by the operation's
/// unit before it opens its own: a batch carries its outermost unit's
/// identity, so it runs in that unit's span, never the operation's.
pub(crate) struct BatchSpan(pub(crate) tracing::Span);

thread_local! {
    /// The [`BatchSpan`] of the request whose loaders are being seeded, for the
    /// spawners built there; set around the seeding alone, which is synchronous.
    static SEEDING: RefCell<Option<tracing::Span>> = const { RefCell::new(None) };
}

/// Holds a request's [`BatchSpan`] in [`SEEDING`] while its loaders are
/// seeded, and restores what was held before on drop, even on a panic.
struct Seeding(Option<tracing::Span>);

impl Seeding {
    fn hold(span: Option<tracing::Span>) -> Self {
        Self(SEEDING.replace(span))
    }
}

impl Drop for Seeding {
    fn drop(&mut self) {
        SEEDING.set(self.0.take());
    }
}

/// What seeds every discovered DataLoader into each GraphQL request.
pub(crate) struct LoaderSeeds {
    container: Container,
    /// The reachable loaders, resolved **once** at schema build.
    seeds: Box<[LoaderSeed]>,
    /// What the batches are carried by — the `/graphql` mount's.
    batches: DetachedWork,
}

/// What the loader registry holds, as a refusal to read it ungated names it.
const LOADERS: &str = "#[dataloader] method";

impl LoaderSeeds {
    /// The loaders the app reaches, or a refusal of a container no app sealed.
    pub(crate) fn new(
        container: Container,
        batches: DetachedWork,
    ) -> Result<Self, UngatedDiscoveryError> {
        let seeds = reachable_seeds(&container)?;
        Ok(Self {
            container,
            seeds,
            batches,
        })
    }

    /// A loader of each owner the app reaches, of the root field `ctx` resolves
    /// alone, its batches in the span its request's run in: a request's loaders
    /// are shared by every field reading through them, so a batch of theirs would
    /// join the keys of root fields whose units hold different abilities. `None`
    /// in a schema no `GraphqlModule` built, which seeds none.
    pub(crate) fn seed_own<T>(ctx: &ContextBase<'_, T>) -> Option<Data> {
        let seeds = ctx.data_opt::<Arc<Self>>()?;
        let mut request = Request::new("");
        if let Some(span) = ctx.data_opt::<BatchSpan>() {
            request = request.data(BatchSpan(span.0.clone()));
        }
        Some(seeds.seed(request).data)
    }

    /// `request`, holding a loader of each owner the app reaches.
    pub(crate) fn seed(&self, mut request: Request) -> Request {
        let held = held_by::<BatchSpan>(&request.data).map(|held| held.0.clone());
        // An ancestor of the current span cannot be entered again, so the
        // spawners read it off the seeding instead.
        let _seeding = Seeding::hold(held);
        for seed in self.seeds.iter() {
            request = seed(&self.container, &self.batches, request);
        }
        request
    }
}

/// Freeze the seeds of the loaders the app reaches.
fn reachable_seeds(container: &Container) -> Result<Box<[LoaderSeed]>, UngatedDiscoveryError> {
    let reachable = ReachableProviders::of(container, LOADERS)?;
    Ok(inventory::iter::<GraphqlLoaderRegistration>()
        .filter(|reg| reachable.reaches((reg.owner_type_id)()))
        .map(|reg| reg.seed)
        .collect())
}

/// Refuse every field the schema serves reading a loader whose owner the app
/// does not reach — each query selecting one would fail — naming the field,
/// what declares it and the module to import; then count at `warn` the
/// unreached loaders no served field reads.
pub(crate) fn check_served_relations(
    container: &Container,
    served: &ServedObjects,
) -> anyhow::Result<()> {
    let reachable = ReachableProviders::of(container, LOADERS)?;
    let mut unreached: Vec<String> = inventory::iter::<GraphqlRelationRegistration>()
        .filter(|relation| !reachable.reaches((relation.owner)().0))
        .filter_map(|relation| {
            let parent = served.serving((relation.parent)(), relation.field)?;
            Some(unreached_relation(container, relation, parent))
        })
        .collect();
    unreached.extend(
        inventory::iter::<GraphqlRootLoaderRegistration>()
            .filter(|read| {
                reachable.reaches((read.resolver)().0) && !reachable.reaches((read.owner)().0)
            })
            .map(|read| unreached_root_read(container, read)),
    );
    if !unreached.is_empty() {
        unreached.sort_unstable();
        anyhow::bail!(
            "a served field reads a loader this app does not reach, so every query \
             selecting it would fail: {}",
            unreached.join("; ")
        );
    }
    warn_unreachable_loaders(&reachable);
    Ok(())
}

/// What a refusal says of one field of the object `parent`: its name, what
/// declares it, the loader's owner and the import its cause prescribes.
fn unreached_relation(
    container: &Container,
    relation: &GraphqlRelationRegistration,
    parent: &str,
) -> String {
    let (owner_type, owner) = (relation.owner)();
    let remedy = import_remedy(container, relation.origin, owner_type, parent);
    format!(
        "`{parent}.{}` (declared on `{}`) reads a loader of `{owner}`: {remedy}",
        relation.field,
        (relation.entity)(),
    )
}

/// What a refusal says of one root field: its name, the resolver declaring it,
/// the loader's owner and the import its cause prescribes.
fn unreached_root_read(container: &Container, read: &GraphqlRootLoaderRegistration) -> String {
    let (owner_type, owner) = (read.owner)();
    let (_, resolver) = (read.resolver)();
    let remedy = import_remedy(container, read.origin, owner_type, resolver);
    format!(
        "`{}` (declared on `{resolver}`) reads a loader of `{owner}`: {remedy}",
        read.field,
    )
}

/// Warn once at schema build about `#[dataloader]`s linked into the binary
/// whose owner's module is unreachable, which no served field reads. Only a
/// count: a registration carries the owner's `TypeId`, not a name.
fn warn_unreachable_loaders(reachable: &ReachableProviders) {
    let skipped = inventory::iter::<GraphqlLoaderRegistration>()
        .filter(|reg| !reachable.reaches((reg.owner_type_id)()))
        .count();
    if skipped > 0 {
        tracing::warn!(
            target: crate::TARGET,
            count = skipped,
            hint = "no field this schema serves reads them; import the modules that provide them if one should",
            "dataloaders linked but unreachable",
        );
    }
}

#[cfg(test)]
mod tests {
    use async_graphql::registry::Registry;

    use super::*;

    #[test]
    fn loaders_seeded_over_a_container_no_app_sealed_are_refused_naming_their_reader() {
        let by_hand = Container::builder().build().expect("the container builds");

        let Err(ungated) = reachable_seeds(&by_hand) else {
            panic!("an ungated discovery is refused, never served empty");
        };

        assert_eq!(ungated.reader, "#[dataloader] method");
    }

    /// The owner of a loader no module provides. Link-time, so it is in every
    /// test of this binary, inert: `reachable_seeds` filters it out.
    struct AbsentOwner;

    inventory::submit! {
        GraphqlLoaderRegistration {
            owner_type_id: || TypeId::of::<AbsentOwner>(),
            seed: |_, _, request| request,
        }
    }

    #[derive(async_graphql::SimpleObject)]
    struct Shelf {
        owner: i32,
    }

    inventory::submit! {
        GraphqlRelationRegistration {
            parent: std::any::type_name::<Shelf>,
            field: "owner",
            entity: || "shelves::Entity",
            owner: || (TypeId::of::<AbsentOwner>(), "AbsentOwner"),
            origin: module_path!(),
        }
    }

    /// An unrelated object spelling the relation's object's GraphQL name, as a
    /// binary links objects its schema does not all serve.
    #[derive(async_graphql::SimpleObject)]
    #[graphql(name = "Shelf")]
    struct NamesakeShelf {
        owner: i32,
    }

    /// What a schema serving only `T` serves.
    fn serving<T: async_graphql::OutputType>() -> ServedObjects {
        let mut registry = Registry::default();
        T::create_type_info(&mut registry);
        ServedObjects(registry)
    }

    #[tokio::test]
    async fn a_field_reading_a_loader_its_request_holds_no_seed_of_fails_and_tells_the_operator() {
        struct Unseeded;

        impl async_graphql::dataloader::Loader<i32> for Unseeded {
            type Value = i32;
            type Error = std::convert::Infallible;

            async fn load(
                &self,
                _: &[i32],
            ) -> Result<std::collections::HashMap<i32, i32>, Self::Error> {
                Ok(std::collections::HashMap::new())
            }
        }

        struct Query;

        #[async_graphql::Object]
        impl Query {
            async fn shelf(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
                seeded_loader::<Unseeded>(ctx).map_err(FieldFailure::into_answer)?;
                Ok(1)
            }
        }

        let logs = nest_rs_testing::LogCapture::install();
        let schema = async_graphql::Schema::new(
            Query,
            async_graphql::EmptyMutation,
            async_graphql::EmptySubscription,
        );

        let answer = schema.execute("{ shelf }").await;

        assert_eq!(
            answer.errors[0].message,
            nest_rs_core::OPAQUE_CLIENT_MESSAGE
        );
        let filed = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert_eq!(filed.level, "error");
        let said = filed.field("error").unwrap_or_default();
        assert!(
            said.contains("`shelf`") && said.contains("Unseeded"),
            "the operator reads the field and the loader: {said}",
        );
    }

    #[test]
    fn a_served_relation_whose_loader_owner_is_unreached_is_refused_naming_the_field() {
        let served = serving::<Shelf>();

        let refused = check_served_relations(&reached(&[]), &served)
            .expect_err("every query selecting `owner` would fail")
            .to_string();

        assert!(
            refused.contains(
                "`Shelf.owner` (declared on `shelves::Entity`) reads a loader of `AbsentOwner`"
            ),
            "{refused}"
        );
        check_served_relations(&reached(&[TypeId::of::<AbsentOwner>()]), &served)
            .expect("its owner reached, the relation is served");
    }

    #[test]
    fn an_object_of_the_same_graphql_name_serves_no_relation_of_another() {
        check_served_relations(&reached(&[]), &serving::<NamesakeShelf>())
            .expect("the schema serves its namesake, whose `owner` reads no loader");
    }

    /// `ReachableProviders` seeded with exactly `types`.
    fn reached(types: &[TypeId]) -> Container {
        Container::builder()
            .provide(nest_rs_core::__private::reachable_providers(
                types.iter().copied(),
            ))
            .build()
            .expect("the container builds")
    }

    #[test]
    fn a_loader_whose_owner_module_is_not_imported_is_counted_at_boot() {
        let logs = nest_rs_testing::LogCapture::install();
        check_served_relations(&reached(&[]), &ServedObjects(Registry::default()))
            .expect("no served field reads it");

        let event = logs.expect_one("nest_rs::graphql", "dataloaders linked but unreachable");
        assert_eq!(event.level, "warn");
        // At least one: the link-time count is a property of the whole test binary.
        let counted: usize = event
            .field("count")
            .and_then(|c| c.parse().ok())
            .unwrap_or_default();
        assert!(
            counted >= 1,
            "the skipped loader is counted: {:?}",
            event.fields,
        );
        assert!(
            event.field("hint").is_some_and(|h| h.contains("import")),
            "the remedy is the point of the line, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn a_loader_whose_owner_is_reachable_is_not_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = reached(&[TypeId::of::<AbsentOwner>()]);
        check_served_relations(&container, &ServedObjects(Registry::default()))
            .expect("no served relation is unreached");
        logs.expect_none("nest_rs::graphql", "dataloaders linked but unreachable");
        let seeds = reachable_seeds(&container).expect("the set is seeded");
        assert!(!seeds.is_empty(), "and it is seeded rather than skipped");
    }

    /// A value the triggering unit carries, read back by the batch.
    #[derive(Debug, PartialEq)]
    struct Caller(&'static str);

    impl nest_rs_core::AmbientValue for Caller {}

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn a_batch_runs_under_what_its_unit_carried_when_it_was_spawned() {
        let spawner = batch_spawner(&DetachedWork::new());
        let correlation = nest_rs_core::Correlation::minted(None);
        let trace = correlation.trace_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        nest_rs_core::with_request_scope(None, correlation, async {
            nest_rs_core::with_ambient(Arc::new(Caller("alice")), async {
                spawner(Box::pin(async move {
                    let _ = tx.send((
                        nest_rs_core::current_trace_id(),
                        nest_rs_core::ambient::<Caller>(),
                    ));
                }));
            })
            .await;
        })
        .await;

        let (seen_trace, seen_caller) = rx.await.expect("the batch runs");
        assert_eq!(seen_trace, Some(trace), "the batch keeps its unit's trace");
        assert_eq!(seen_caller.as_deref(), Some(&Caller("alice")));
    }
}
