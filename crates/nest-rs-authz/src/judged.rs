//! A wire row judged where `#[expose]`'s conversion builds it: the ability installed
//! there decides which rows and which columns the caller may read, a withheld column
//! is never in the row, and the row names who judged it, so it answers no one else.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::LazyLock;

use nest_rs_core::__private::{
    AnswerRefusal, Audience, RefusedAnswer, Site, WireRow, audience, refuse_answer,
};
use nest_rs_core::{Handler, ServedAddress};
use nest_rs_database::{ExecutorScope, current_executor_scope};
use sea_orm::{EntityTrait, IdenStatic};

use crate::ability::{FieldSet, Provenance};
use crate::predicate::nothing_invented;
use crate::{Ability, Action};

/// Who judged a row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Judge {
    /// The ability installed where the row was built, by its id.
    Ability(u64),
    /// System work, a job holding no ability: every exposed column, for no caller.
    System,
    /// Nothing: a unit holding no ability outside a job, or no unit at all.
    Nobody,
}

/// What every wire row `#[expose]` builds carries: who judged it, and whether the
/// caller may read it at all. No public constructor, so a struct literal cannot
/// build a wire row a judgement did not.
#[derive(Clone, Copy)]
pub struct Judged {
    judge: Judge,
    refused: bool,
}

impl fmt::Debug for Judged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match (self.judge, self.refused) {
            (_, true) => "Judged(refused)",
            (Judge::Ability(_), false) => "Judged(caller)",
            (Judge::System, false) => "Judged(system)",
            (Judge::Nobody, false) => "Judged(nobody)",
        })
    }
}

/// The verdict a wire row is built from: the columns it may hold, and its proof.
pub struct Judgement {
    fields: FieldSet,
    judged: Judged,
}

impl Judgement {
    /// Whether the row holds `column`, which its grants may withhold.
    pub fn permits<C: IdenStatic>(&self, column: C) -> bool {
        !self.judged.refused && self.fields.permits(column.as_str())
    }

    /// The proof the row carries.
    pub fn proof(&self) -> Judged {
        self.judged
    }
}

/// Judge `model`, a row of `E`, for the caller the running unit answers: by the
/// ability installed there, at `Read`, as a row a read through `Repo` under it
/// admitted — a rule the row cannot decide counts as SQL counted it. A job holding
/// no ability is system work and reads every exposed column; anywhere else no
/// ability judges, and the row answers no one.
pub fn judge<E: EntityTrait>(model: &E::Model) -> Judgement {
    match crate::context::installed() {
        Some(ability) => judged_by::<E>(&ability, model),
        None if current_executor_scope() == Some(ExecutorScope::Job) => Judgement {
            fields: FieldSet::All,
            judged: Judged {
                judge: Judge::System,
                refused: false,
            },
        },
        None => Judgement {
            fields: FieldSet::Only(HashSet::new()),
            judged: Judged {
                judge: Judge::Nobody,
                refused: true,
            },
        },
    }
}

/// [`judge`] by `ability`, as a row a read under it admitted, or as a push.
fn judged_by<E: EntityTrait>(ability: &Ability, model: &E::Model) -> Judgement {
    let provenance = if nest_rs_core::__private::is_pushed() {
        Provenance::Pushed
    } else {
        Provenance::Loaded
    };
    let verdict = ability.evaluate::<E>(Action::Read, model, provenance, &nothing_invented);
    Judgement {
        fields: verdict.fields,
        judged: Judged {
            judge: Judge::Ability(ability.id()),
            refused: !verdict.allowed,
        },
    }
}

/// Why a wire row refused to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refused {
    /// The caller's grants do not let it read this row.
    Withheld,
    /// Judged under another ability than the one answering: another caller's row,
    /// a cache, a value carried out of the unit that built it.
    JudgedForAnother,
    /// Judged under a caller's ability, and pushed to other connections.
    PushedToOthers,
    /// Built where no ability judged it.
    JudgedByNothing,
}

impl Refused {
    /// The value an incident query groups on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Withheld => "row_not_granted",
            Self::JudgedForAnother => "judged_for_another",
            Self::PushedToOthers => "pushed_to_others",
            Self::JudgedByNothing => "judged_by_nothing",
        }
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Withheld => "a row the caller may not read",
            Self::JudgedForAnother => "a row judged for another caller",
            Self::PushedToOthers => "a row judged for its caller, pushed to other connections",
            Self::JudgedByNothing => "a row no ability judged",
        })
    }
}

impl Judged {
    /// Whether this row answers the caller the running unit answers: the ability
    /// that judged it is the one installed, and a push to other connections takes a
    /// system row alone. A system row — a job's — answers inside a job, where no
    /// caller is, and nowhere else: carried out of it, it is another caller's.
    fn answers(self) -> Result<(), Refused> {
        match (self.judge, crate::context::installed_id()) {
            (Judge::Nobody, _) => Err(Refused::JudgedByNothing),
            (Judge::Ability(_), _) if audience() == Audience::Others => {
                Err(Refused::PushedToOthers)
            }
            (Judge::Ability(by), Some(answering)) if by == answering => {
                if self.refused {
                    Err(Refused::Withheld)
                } else {
                    Ok(())
                }
            }
            (Judge::System, _) if audience() == Audience::Others => Ok(()),
            (Judge::System, None) if current_executor_scope() == Some(ExecutorScope::Job) => Ok(()),
            _ => Err(Refused::JudgedForAnother),
        }
    }

    /// Whether a row of `E` may be written for the caller it answers, every column at
    /// once, as a serializer writes it; a refusal is handed to the edge answering,
    /// which files it once at the unit it names, since a serializer stops at it.
    pub fn serialize_check<E: EntityTrait>(&self) -> Result<(), Refused> {
        self.answers()
            .inspect_err(|&refused| refuse_answer(refused_answer::<E>(refused)))
    }

    /// Why a row of `E` refuses the caller it answers, every column at once, as a
    /// field resolving under it reads them; `None` when it answers.
    pub fn refusal<E: EntityTrait>(&self) -> Option<RefusedAnswer> {
        self.answers().err().map(refused_answer::<E>)
    }
}

/// `refused` as the edge answering hands it on.
fn refused_answer<E: EntityTrait>(refused: Refused) -> RefusedAnswer {
    RefusedAnswer::new(
        match refused {
            Refused::Withheld => AnswerRefusal::Withheld,
            _ => AnswerRefusal::Unjudged,
        },
        E::default().table_name(),
        refused.as_str(),
        file,
    )
}

/// The line a refused row files, naming the unit as its `mounted` line does: a
/// denial at `warn`, a row that reached a caller it was not judged for, a wiring
/// defect, at `error`.
fn file(refused: &RefusedAnswer, handler: Option<&Handler>, served: ServedAddress<'_>) {
    let host = handler.map(Handler::host_name);
    let method = handler.map(Handler::method);
    macro_rules! filed {
        ($level:ident, $said:literal) => {
            tracing::$level!(
                target: crate::TARGET,
                entity = refused.entity(),
                reason = refused.reason(),
                host,
                method,
                verb = served.verb,
                path = served.path,
                kind = served.kind,
                field = served.field,
                event = served.event,
                tool = served.tool,
                prompt = served.prompt,
                resource = served.resource,
                $said,
            )
        };
    }
    match refused.refusal() {
        AnswerRefusal::Withheld => filed!(warn, "a reply's row is withheld from its caller"),
        _ => filed!(error, "a wire row answered a caller it was not judged for"),
    }
}

/// The guard `#[expose(…, graphql)]` puts on each column of a wire type of `E`: the
/// column resolves only for the caller the row was judged for. `key` is the primary
/// key's, which a row the caller's grants refuse still answers in a mutation's reply
/// alone — a write names the row it wrote (F6) — and nowhere else, so a query or a
/// push never hands a caller the key of a row it may not read. A refusal is filed
/// once per operation and reason, however many rows and columns repeat it.
#[cfg(feature = "graphql")]
pub fn judged_field<E: EntityTrait>(
    judged: Judged,
    key: bool,
) -> impl Fn(&nest_rs_graphql::async_graphql::Context<'_>) -> nest_rs_graphql::async_graphql::Result<()>
+ Send
+ Sync
+ 'static {
    use nest_rs_graphql::async_graphql::parser::types::OperationType;

    move |ctx| {
        let refused = match judged.answers() {
            Ok(()) => return Ok(()),
            Err(Refused::Withheld)
                if key && ctx.query_env.operation.node.ty == OperationType::Mutation =>
            {
                return Ok(());
            }
            Err(refused) => refused,
        };
        nest_rs_graphql::__private::file_below_root(
            ctx,
            &refused_answer::<E>(refused),
            nest_rs_graphql::__private::root_site(ctx),
        );
        Err(match refused {
            Refused::Withheld => crate::graphql::forbidden(),
            _ => nest_rs_graphql::denial_to_graphql_error(nest_rs_guards::Denial::internal(
                "a wire row answered a caller it was not judged for",
            )),
        })
    }
}

/// A wire row a push may carry: implemented by `#[expose]` on its wire type, never
/// by hand; local, so a list of rows is told from a row.
pub trait JudgedRow: WireRow {}

/// What a subscription's item holds that the stream can tell: a wire row, a list
/// of them, or either inside an `Option` or a `Result`, the shapes an item of rows
/// takes. Implemented here alone: a downstream crate can name none of them.
pub trait PushedRows: Sized {
    /// The item, where it answers the subscriber; `None` takes it off the stream.
    fn answering(self, site: &'static Site) -> Option<Self>;
}

impl<T: JudgedRow> PushedRows for T {
    fn answering(self, site: &'static Site) -> Option<Self> {
        answers_subscriber(&self, site).then_some(self)
    }
}

/// A list keeps the rows that answer.
impl<T: JudgedRow> PushedRows for Vec<T> {
    fn answering(mut self, site: &'static Site) -> Option<Self> {
        self.retain(|row| answers_subscriber(row, site));
        Some(self)
    }
}

/// `None` passes; a row inside that does not answer takes the item off.
impl<T: PushedRows> PushedRows for Option<T> {
    fn answering(self, site: &'static Site) -> Option<Self> {
        match self {
            Some(held) => held.answering(site).map(Some),
            None => Some(None),
        }
    }
}

/// An error passes; a row inside an `Ok` that does not answer takes the item off.
impl<T: PushedRows, E> PushedRows for Result<T, E> {
    fn answering(self, site: &'static Site) -> Option<Self> {
        match self {
            Ok(held) => held.answering(site).map(Ok),
            Err(failed) => Some(Err(failed)),
        }
    }
}

/// One item a subscription's stream yields, kept only where it answers the
/// subscriber: a wire row its judgement refuses, or one judged for another
/// caller, leaves the stream wherever [`PushedRows`] finds it, and a list keeps
/// the rows that answer. Any other item passes, and its rows refuse every column
/// where they resolve, the key included.
pub struct PushedItem<T>(pub T);

impl<T: PushedRows> PushedItem<T> {
    /// The item, where it answers the subscriber; `site` is the subscription's.
    pub fn kept(self, site: &'static Site) -> Option<T> {
        self.0.answering(site)
    }
}

/// What [`PushedItem::kept`] answers for an item whose rows [`PushedRows`] cannot
/// tell.
pub trait PushedItemFallback<T> {
    /// The item, whole.
    fn kept(self, site: &'static Site) -> Option<T>;
}

impl<T> PushedItemFallback<T> for PushedItem<T> {
    fn kept(self, _site: &'static Site) -> Option<T> {
        Some(self.0)
    }
}

/// Whether a pushed row answers the subscriber polling it: a refusal its grants
/// make is said at `debug`, every subscriber reading its own; a row judged for
/// another caller, a defect of the push, at `error`. Each names the subscription
/// as its `mounted` line does.
fn answers_subscriber<T: JudgedRow>(row: &T, site: &'static Site) -> bool {
    let Some(refused) = row.refusal() else {
        return true;
    };
    let handler = site.handler();
    let (host, method, field) = (handler.host_name(), handler.method(), handler.address());
    if refused.refusal() == AnswerRefusal::Withheld {
        tracing::debug!(
            target: crate::TARGET,
            host,
            method,
            kind = "subscription",
            field,
            reason = refused.reason(),
            "subscription item withheld",
        );
    } else {
        tracing::error!(
            target: crate::TARGET,
            host,
            method,
            kind = "subscription",
            field,
            reason = refused.reason(),
            "a wire row answered a caller it was not judged for",
        );
    }
    false
}

/// The columns of an `#[expose]`d entity a reply's grant must not withhold: those
/// its wire type cannot answer null, each as `Column::as_str()` names it beside the
/// field declaring it. Submitted by `#[expose]`, never by hand.
pub struct WireColumns {
    /// The entity's `TypeId`.
    pub entity: fn() -> TypeId,
    /// Each column no grant may withhold, and its field's name.
    pub answered: fn() -> Vec<(&'static str, &'static str)>,
}

inventory::collect!(WireColumns);

/// Each `#[expose]`d entity's columns no grant may withhold, beside their fields.
type Answered = HashMap<TypeId, Vec<(&'static str, &'static str)>>;

/// Every `#[expose]`d entity's columns no grant may withhold, read once.
static ANSWERED: LazyLock<Answered> = LazyLock::new(|| {
    inventory::iter::<WireColumns>
        .into_iter()
        .map(|columns| ((columns.entity)(), (columns.answered)()))
        .collect()
});

/// The first column of `subject` a reply's grant permitting `fields` withholds and
/// its wire type cannot answer null, beside the field declaring it.
pub(crate) fn unanswerable(
    subject: TypeId,
    fields: &FieldSet,
) -> Option<(&'static str, &'static str)> {
    ANSWERED
        .get(&subject)?
        .iter()
        .copied()
        .find(|(column, _)| !fields.permits(column))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_database::__private::{with_job_executor, with_request_executor};
    use nest_rs_database::Executor;

    use super::*;
    use crate::AbilityBuilder;
    use crate::context::install_ability;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub note: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    struct Handle;

    impl Executor for Handle {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn row(org_id: i32) -> widget::Model {
        widget::Model {
            id: 1,
            org_id,
            note: "kept".into(),
        }
    }

    fn reading(build: impl FnOnce(&mut AbilityBuilder)) -> Arc<Ability> {
        let mut ab = AbilityBuilder::new();
        build(&mut ab);
        Arc::new(ab.build().expect("a valid rule set"))
    }

    #[tokio::test]
    async fn a_row_is_judged_by_the_ability_installed_where_it_is_built() {
        let ability = reading(|ab| {
            ab.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::OrgId, 7))
                .fields([widget::Column::Id, widget::Column::OrgId]);
        });
        let judged = install_ability(ability, async {
            let judgement = judge::<widget::Entity>(&row(7));
            assert!(judgement.permits(widget::Column::OrgId));
            assert!(!judgement.permits(widget::Column::Note));
            assert_eq!(judgement.proof().answers(), Ok(()));
            judgement.proof()
        })
        .await;
        assert_eq!(judged.answers(), Err(Refused::JudgedForAnother));
    }

    #[tokio::test]
    async fn a_row_no_grant_admits_is_withheld_whole() {
        let ability = reading(|ab| {
            ab.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::OrgId, 7));
        });
        install_ability(ability, async {
            let judgement = judge::<widget::Entity>(&row(8));
            assert!(!judgement.permits(widget::Column::Id));
            assert!(!judgement.permits(widget::Column::Note));
            assert_eq!(judgement.proof().answers(), Err(Refused::Withheld));
        })
        .await;
    }

    #[tokio::test]
    async fn another_ability_of_the_same_rules_is_another_caller() {
        let rules = |ab: &mut AbilityBuilder| {
            ab.can(Action::Read, widget::Entity);
        };
        let judged = install_ability(reading(rules), async {
            judge::<widget::Entity>(&row(7)).proof()
        })
        .await;
        install_ability(reading(rules), async move {
            assert_eq!(judged.answers(), Err(Refused::JudgedForAnother));
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_reads_every_column_and_answers_inside_a_job_and_on_a_push_alone() {
        let judged = with_job_executor(Arc::new(Handle), async {
            let judgement = judge::<widget::Entity>(&row(7));
            assert!(judgement.permits(widget::Column::Note));
            judgement.proof()
        })
        .await;
        let in_a_job = with_job_executor(Arc::new(Handle), async { judged.answers() }).await;
        assert_eq!(in_a_job, Ok(()));
        assert_eq!(
            judged.answers(),
            Err(Refused::JudgedForAnother),
            "carried out of every unit"
        );
        let in_a_request =
            with_request_executor(Arc::new(Handle), async { judged.answers() }).await;
        assert_eq!(
            in_a_request,
            Err(Refused::JudgedForAnother),
            "a request holding no ability is a caller all the same"
        );
        assert_eq!(
            nest_rs_core::__private::for_others(|| judged.answers()),
            Ok(())
        );
        let caller = reading(|ab| {
            ab.can(Action::Read, widget::Entity);
        });
        install_ability(caller, async move {
            assert_eq!(judged.answers(), Err(Refused::JudgedForAnother));
            assert_eq!(
                nest_rs_core::__private::for_others(|| judged.answers()),
                Ok(())
            );
        })
        .await;
    }

    #[tokio::test]
    async fn a_request_holding_no_ability_judges_nothing() {
        let judged = with_request_executor(Arc::new(Handle), async {
            judge::<widget::Entity>(&row(7)).proof()
        })
        .await;
        assert_eq!(judged.answers(), Err(Refused::JudgedByNothing));
        assert_eq!(
            judge::<widget::Entity>(&row(7)).proof().answers(),
            Err(Refused::JudgedByNothing)
        );
    }

    #[tokio::test]
    async fn a_callers_row_pushed_to_other_connections_refuses() {
        let ability = reading(|ab| {
            ab.can(Action::Read, widget::Entity);
        });
        install_ability(ability, async {
            let judged = judge::<widget::Entity>(&row(7)).proof();
            assert_eq!(judged.answers(), Ok(()));
            assert_eq!(
                nest_rs_core::__private::for_others(|| judged.answers()),
                Err(Refused::PushedToOthers)
            );
        })
        .await;
    }

    /// A grant comparing two value types only the database compares: a read under it
    /// was filtered by SQL, a push was not.
    #[tokio::test]
    async fn a_rule_no_push_can_decide_grants_it_nothing() {
        let ability = reading(|ab| {
            ab.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Note, 7));
        });
        install_ability(ability, async {
            assert_eq!(judge::<widget::Entity>(&row(7)).proof().answers(), Ok(()));
            let pushed =
                nest_rs_core::__private::as_pushed(|| judge::<widget::Entity>(&row(7)).proof());
            assert_eq!(pushed.answers(), Err(Refused::Withheld));
            assert_eq!(judge::<widget::Entity>(&row(7)).proof().answers(), Ok(()));
        })
        .await;
    }

    #[test]
    fn a_refusal_names_no_value() {
        let printed = format!(
            "{:?}",
            Judged {
                judge: Judge::Ability(3),
                refused: false
            }
        );
        assert_eq!(printed, "Judged(caller)");
    }
}
