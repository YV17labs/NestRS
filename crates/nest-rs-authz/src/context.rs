//! The caller's [`Ability`] as a value of the ambient context: the ability
//! guard attaches it for the unit, `nest-rs-seaorm`'s `Repo` reads it back.

use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::UnmaskedReply;
use nest_rs_core::{AmbientValue, UnitContext};
use nest_rs_database::__private::installed_scope;
use nest_rs_database::{ExecutorScope, current_executor_scope};

use crate::Ability;

/// The ability a unit runs under as the ambient context holds it — a type only
/// this crate builds, so no `with_ambient` or `cx.attach` elsewhere puts a
/// wider one there. `None` where [`with_ability`] had nothing to narrow.
pub struct AmbientAbility {
    ability: Option<Arc<Ability>>,
    /// What this holder's request already filed. One request holds one holder, where
    /// an [`Ability`] may serve every request of a role.
    #[cfg(feature = "graphql")]
    filed: crate::graphql::FiledRefusals,
}

impl AmbientAbility {
    fn holding(ability: Option<Arc<Ability>>) -> Self {
        Self {
            ability,
            #[cfg(feature = "graphql")]
            filed: crate::graphql::FiledRefusals::default(),
        }
    }

    /// `ability`, held as the ability guard attaches the caller's.
    pub fn new(ability: impl Into<Arc<Ability>>) -> Self {
        Self::holding(Some(ability.into()))
    }

    /// The ability held, or `None` where nothing was narrowed.
    pub fn ability(&self) -> Option<Arc<Ability>> {
        self.ability.clone()
    }
}

/// The caller's rules travel wherever the unit continues, and never into a
/// job's envelope: a job is system work.
impl AmbientValue for AmbientAbility {
    /// Re-entered inside another unit, a capture's ability is narrowed to the
    /// one that unit reads under, kept where that unit is a job or no unit at
    /// all — the capture continuing its own — and dropped in a request reading
    /// under none, whose reads stay closed.
    fn reenter(self: Arc<Self>) -> Option<Arc<Self>> {
        let Some(captured) = self.ability() else {
            return Some(self);
        };
        match current_ability() {
            Some(held) if Arc::ptr_eq(&held, &captured) => Some(self),
            Some(held) => Some(Arc::new(Self::new(held.intersection(&captured)))),
            None => (installed_scope(ExecutorScope::Job) == ExecutorScope::Job).then_some(self),
        }
    }
}

/// Whether the running request has yet to file this refusal. Where no ability is
/// ambient, each files its own.
#[cfg(feature = "graphql")]
pub(crate) fn first_refusal(subject: &'static str, action: crate::Action, fields: &str) -> bool {
    nest_rs_core::ambient::<AmbientAbility>()
        .is_none_or(|held| held.filed.first(subject, action, fields))
}

/// The ability installed for the running unit, withheld from its reads or not.
pub(crate) fn installed() -> Option<Arc<Ability>> {
    nest_rs_core::ambient::<AmbientAbility>().and_then(|held| held.ability())
}

/// The ability the unit `cx` runs carries: attached earlier in its stage, else
/// ambient.
pub(crate) fn carried(cx: &UnitContext<'_>) -> Option<Arc<Ability>> {
    cx.ambient::<AmbientAbility>()
        .and_then(|held| held.ability())
}

/// The [`Ability`] the running unit's reads run under, or `None` outside a
/// request, in a request that runs no authorization, on a unit whose reply no
/// response mask shapes, and for a visitor anywhere but a unit naming what it
/// reads: there the ability the guard attached is withheld, so a read the mask
/// would have scoped fails closed instead of answering unmasked. A job withholds
/// nothing: `None` there reads as system work, wider than what it holds.
pub fn current_ability() -> Option<Arc<Ability>> {
    let held = installed()?;
    let withheld = nest_rs_core::ambient::<UnmaskedReply>().is_some()
        || (held.is_visitor() && nest_rs_core::ambient::<VisitorReadNamed>().is_none());
    (!withheld || current_executor_scope() == Some(ExecutorScope::Job)).then_some(held)
}

/// Whether [`current_ability`] answers none because of a visitor's ability the
/// running unit holds off the unit naming its read: a nested GraphQL field, a
/// relation's batch. A unit whose own posture names no read says so by
/// [`UnmaskedReply`], and a line there has a posture to ask for.
pub fn visitors_read_unnamed() -> bool {
    installed().is_some_and(|held| held.is_visitor())
        && nest_rs_core::ambient::<VisitorReadNamed>().is_none()
        && nest_rs_core::ambient::<UnmaskedReply>().is_none()
}

/// Ambient on a unit whose `#[public(Action, Entity)]` admitted a visitor: what
/// that visitor reads is named, so its rows and its fields are its grant's. What
/// runs off that unit, a nested GraphQL field or a relation's batch, names nothing,
/// and a visitor's ability is withheld there.
pub(crate) struct VisitorReadNamed;

impl AmbientValue for VisitorReadNamed {}

/// Run `fut` under `ability` narrowed to what the running unit already runs
/// under, never wider.
///
/// Inside a unit holding an ability — a request its guard authorized, a job
/// already narrowed — `fut` runs under what both allow. A job holds none and
/// reads unscoped, so there `fut` runs under `ability`: a job narrowing its
/// system work to a principal's. Anywhere else — a request no ability guard
/// authorized, outside every unit — nothing is installed, and a read that
/// failed closed still does.
///
/// No public function widens it: the ability is ambient through a type only
/// this crate builds, so the kernel's own install refuses one.
///
/// ```compile_fail,E0277
/// # use std::sync::Arc;
/// # use nest_rs_authz::AbilityBuilder;
/// # let ability = AbilityBuilder::new().build()?;
/// let widened = nest_rs_core::with_ambient(Arc::new(ability), async {});
/// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
/// ```
pub fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> impl Future<Output = F::Output> {
    nest_rs_core::__private::with_ambient_from(
        move || Arc::new(AmbientAbility::holding(narrowed(ability))),
        fut,
    )
}

/// What [`with_ability`] installs where it runs: the intersection with the
/// ability held, `ability` in a job holding none, and nothing elsewhere.
fn narrowed(ability: Arc<Ability>) -> Option<Arc<Ability>> {
    match installed() {
        Some(held) if Arc::ptr_eq(&held, &ability) => Some(held),
        Some(held) => Some(Arc::new(held.intersection(&ability))),
        None => (current_executor_scope() == Some(ExecutorScope::Job)).then_some(ability),
    }
}

/// Run `fut` under `ability`, in place of whatever the unit carries: the
/// testing harness's request unit, and a framework suite's. It stands in for a
/// unit naming its read, so a visitor's reads are licensed too.
pub fn install_ability<F: Future>(
    ability: Arc<Ability>,
    fut: F,
) -> impl Future<Output = F::Output> {
    nest_rs_core::with_ambient(
        Arc::new(AmbientAbility::new(ability)),
        nest_rs_core::with_ambient(Arc::new(VisitorReadNamed), fut),
    )
}

#[cfg(test)]
mod tests {
    use std::any::{Any, TypeId};

    use nest_rs_core::{Continuation, RequestContinuation};
    use nest_rs_database::__private::{with_job_executor, with_request_executor};
    use nest_rs_database::Executor;

    use super::*;
    use crate::{AbilityBuilder, Action};

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    fn ability() -> Arc<Ability> {
        Arc::new(
            AbilityBuilder::new()
                .build()
                .expect("an empty rule set builds"),
        )
    }

    /// An ability reading every widget.
    fn reading() -> Arc<Ability> {
        let mut ab = AbilityBuilder::new();
        ab.can(Action::Read, widget::Entity);
        Arc::new(ab.build().expect("a valid rule set"))
    }

    /// Whether the ability the running unit reads under may read widgets, or
    /// `None` when it runs under none.
    fn reads_widgets() -> Option<bool> {
        current_ability()
            .map(|ability| ability.can_class(Action::Read, TypeId::of::<widget::Entity>()))
    }

    struct Handle;

    impl Executor for Handle {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn a_request_holding_no_ability_is_given_none() {
        let read = with_request_executor(
            Arc::new(Handle),
            with_ability(reading(), async { reads_widgets() }),
        )
        .await;
        assert_eq!(read, None, "a request's reads fail closed, and stay so");
    }

    #[tokio::test]
    async fn no_unit_is_given_an_ability() {
        assert_eq!(
            with_ability(reading(), async { reads_widgets() }).await,
            None
        );
    }

    #[tokio::test]
    async fn a_job_narrows_its_unscoped_reads_to_the_ability_it_is_handed() {
        let narrowed = reading();
        let installed = with_job_executor(
            Arc::new(Handle),
            with_ability(Arc::clone(&narrowed), async { current_ability() }),
        )
        .await;
        assert!(installed.is_some_and(|installed| Arc::ptr_eq(&installed, &narrowed)));
    }

    /// A visitor's ability, which reads no widget.
    fn visitor() -> Arc<Ability> {
        Arc::new(
            AbilityBuilder::new()
                .build_visitor()
                .expect("an empty rule set builds"),
        )
    }

    #[tokio::test]
    async fn a_job_narrowed_to_a_visitor_reads_under_the_visitor() {
        let narrowed = visitor();
        let installed = with_job_executor(
            Arc::new(Handle),
            with_ability(Arc::clone(&narrowed), async { current_ability() }),
        )
        .await;
        assert!(
            installed.is_some_and(|installed| Arc::ptr_eq(&installed, &narrowed)),
            "none in a job reads as system work, every row",
        );
    }

    #[tokio::test]
    async fn a_request_unit_handed_a_visitor_by_the_harness_reads_under_the_visitor() {
        let handed = visitor();
        let installed = with_request_executor(
            Arc::new(Handle),
            install_ability(Arc::clone(&handed), async { current_ability() }),
        )
        .await;
        assert!(installed.is_some_and(|installed| Arc::ptr_eq(&installed, &handed)));
    }

    #[tokio::test]
    async fn a_visitor_a_request_holds_outside_a_named_read_is_withheld() {
        let read = with_request_executor(
            Arc::new(Handle),
            nest_rs_core::with_ambient(Arc::new(AmbientAbility::new(visitor())), async {
                current_ability().is_some()
            }),
        )
        .await;
        assert!(!read, "no unit named what this visitor reads");
    }

    #[tokio::test]
    async fn a_unit_holding_an_ability_runs_under_what_both_allow() {
        let read = with_job_executor(
            Arc::new(Handle),
            with_ability(
                ability(),
                with_ability(reading(), async { reads_widgets() }),
            ),
        )
        .await;
        assert_eq!(read, Some(false), "the held ability reads no widget");
    }

    #[test]
    fn an_install_holds_the_unit_it_runs_once() {
        let heavy = || async {
            let held = [0u8; 32 * 1024];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
        };
        let alone = std::mem::size_of_val(&heavy());
        let size = std::mem::size_of_val(&with_ability(ability(), heavy()));
        assert!(
            size < alone + 1024,
            "with_ability weighs {size} bytes around a {alone}-byte unit",
        );
    }

    #[tokio::test]
    async fn the_ability_crosses_every_continuation_but_a_jobs_envelope() {
        let ability = ability();
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Task,
            Continuation::Commit,
            Continuation::Connection,
            Continuation::Envelope,
        ] {
            let carried = install_ability(Arc::clone(&ability), async {
                RequestContinuation::capture(into)
            })
            .await
            .expect("installed");
            let crossed = carried.enter(current_ability);
            match into {
                Continuation::Envelope => assert!(crossed.is_none(), "a job is system work"),
                _ => assert!(
                    crossed.is_some_and(|crossed| Arc::ptr_eq(&crossed, &ability)),
                    "{into:?} carries the caller's own ability",
                ),
            }
        }
    }

    /// What a request holding `reading()` carries into its commit.
    async fn a_readers_commit() -> RequestContinuation {
        with_request_executor(
            Arc::new(Handle),
            install_ability(reading(), async {
                RequestContinuation::capture(Continuation::Commit)
            }),
        )
        .await
        .expect("a unit runs")
    }

    #[tokio::test]
    async fn a_capture_re_entered_inside_another_unit_reads_under_what_both_allow() {
        let captured = a_readers_commit().await;
        let read = |held: Arc<Ability>| {
            with_request_executor(
                Arc::new(Handle),
                install_ability(held, captured.scope(async { reads_widgets() })),
            )
        };

        assert_eq!(read(reading()).await, Some(true));
        assert_eq!(
            read(ability()).await,
            Some(false),
            "the running unit reads no widget"
        );
        assert_eq!(
            with_request_executor(Arc::new(Handle), captured.scope(async { reads_widgets() }))
                .await,
            None,
            "a request holding none stays closed",
        );
        assert_eq!(
            with_job_executor(Arc::new(Handle), captured.scope(async { reads_widgets() })).await,
            Some(true),
            "a job reads under the principal its capture carries",
        );
        assert_eq!(
            captured.scope(async { reads_widgets() }).await,
            Some(true),
            "outside every unit, the capture continues its own",
        );
    }

    #[tokio::test]
    async fn a_capture_re_entered_in_a_job_narrowed_to_a_visitor_reads_under_what_both_allow() {
        let captured = a_readers_commit().await;
        let read = with_job_executor(
            Arc::new(Handle),
            with_ability(visitor(), captured.scope(async { reads_widgets() })),
        )
        .await;
        assert_eq!(read, Some(false), "the visitor reads no widget");
    }

    /// Whether a job unit a task opens, carrying `captured`, may read widgets.
    async fn a_tasks_job_reads_widgets(captured: Option<RequestContinuation>) -> Option<bool> {
        captured
            .expect("a unit runs")
            .scope(with_job_executor(Arc::new(Handle), async {
                reads_widgets()
            }))
            .await
    }

    #[tokio::test]
    async fn a_job_a_withholding_units_task_opens_reads_under_what_the_task_carries() {
        let capture = || async { RequestContinuation::capture(Continuation::Task) };

        let unmasked = with_request_executor(
            Arc::new(Handle),
            install_ability(
                reading(),
                nest_rs_core::with_ambient(Arc::new(UnmaskedReply), capture()),
            ),
        )
        .await;
        assert_eq!(
            a_tasks_job_reads_widgets(unmasked).await,
            Some(true),
            "none in a job reads as system work, wider than the unit it came off",
        );

        let visitor = with_request_executor(
            Arc::new(Handle),
            nest_rs_core::with_ambient(Arc::new(AmbientAbility::new(visitor())), capture()),
        )
        .await;
        assert_eq!(
            a_tasks_job_reads_widgets(visitor).await,
            Some(false),
            "none in a job reads as system work, wider than the visitor it came off",
        );
    }

    #[tokio::test]
    async fn a_capture_re_entered_where_no_mask_shapes_the_reply_reads_nothing() {
        let captured = a_readers_commit().await;
        let read = with_request_executor(
            Arc::new(Handle),
            install_ability(
                reading(),
                nest_rs_core::with_ambient(
                    Arc::new(UnmaskedReply),
                    captured.scope(async { reads_widgets() }),
                ),
            ),
        )
        .await;
        assert_eq!(read, None, "the running route withholds its ability");
    }
}
