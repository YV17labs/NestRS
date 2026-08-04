//! Opt-in soft-delete markers and read filters.
//!
//! Entities declare [`SoftDeletable`] via `#[expose(..., soft_delete)]`; services
//! opt in through [`CrudService::soft_delete_column`](crate::CrudService::soft_delete_column).
//! Hand-written queries that bypass `CrudService` should AND
//! [`live_condition`](live_condition) onto [`Repo::scoped`](crate::Repo::scoped).
//!
//! **The two halves are checked at boot.** An entity carrying the flag whose
//! service never overrides the column is not a half-configured feature — it is
//! an irreversible one: `DELETE` erases the row for good, reads never filter
//! `deleted_at`, and the wire response is byte-for-byte the one a successful
//! tombstone returns. `#[expose]` submits the pair to
//! [`SoftDeleteRegistration`] at link time and [`SoftDeleteAudit`] refuses boot
//! on a mismatch, which is the only moment both facts are knowable without a
//! row having already been destroyed.

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, EntityTrait};

/// Marker for entities with a nullable `deleted_at` tombstone column. Emitted by
/// `#[expose(..., soft_delete)]`; the service still opts in via
/// [`CrudService::soft_delete_column`](crate::CrudService::soft_delete_column).
pub trait SoftDeletable: EntityTrait {
    /// The nullable tombstone column (`deleted_at`) whose non-null value marks a
    /// row as soft-deleted.
    fn deleted_at_column() -> Self::Column;
}

/// `deleted_at IS NULL` for a [`SoftDeletable`] entity — AND this onto any custom
/// [`Repo::scoped`](crate::Repo::scoped) query so tombstones stay invisible.
pub fn live_condition<E: SoftDeletable>() -> Condition {
    live_condition_for_column(E::deleted_at_column())
}

/// The `<col> IS NULL` live-row predicate, built from a tombstone column — the
/// single source of "what a live row looks like" shared by [`live_condition`]
/// and `CrudService::live_read_filter`.
pub(crate) fn live_condition_for_column<C: ColumnTrait>(col: C) -> Condition {
    Condition::all().add(col.is_null())
}

/// One `#[expose(..., soft_delete)]` entity and the service it named, submitted
/// at link time so [`SoftDeleteAudit`] can compare the entity's half against the
/// service's.
///
/// Every field is a fn pointer rather than a value: `table_name()`,
/// `type_name::<S>()` and `soft_delete_column()` are all calls, and the entry
/// has to be constructible in a `static`. Emitted by the decorator, never
/// written by hand.
pub struct SoftDeleteRegistration {
    /// The entity's table name — what the audit reports, and what
    /// `CrudService::entity_name` logs.
    pub entity: fn() -> &'static str,
    /// The service `#[expose(service = …)]` named, for the message.
    pub service: fn() -> &'static str,
    /// `<Service as CrudService>::soft_delete_column().is_some()` — the half the
    /// entity flag cannot see.
    pub tombstones: fn() -> bool,
}

inventory::collect!(SoftDeleteRegistration);

/// The boot refusal for a half-wired tombstone.
///
/// A lifecycle hook rather than a `DatabaseModule` factory for the same reason
/// `AudienceBinding` is one: it depends on nothing built in the collect phase,
/// so running it after every provider exists makes the answer independent of
/// import order — and an `Err` from `#[on_module_init]` aborts boot.
#[nest_rs_core::injectable]
#[derive(Default)]
pub(crate) struct SoftDeleteAudit;

#[nest_rs_core::hooks]
impl SoftDeleteAudit {
    #[on_module_init]
    async fn verify(&self) -> anyhow::Result<()> {
        audit_soft_delete_bindings()
    }
}

/// Run the soft-delete audit over every `#[expose(..., soft_delete)]` entity
/// linked into this binary.
///
/// [`DatabaseModule`](crate::DatabaseModule) runs it at boot; it is public so an
/// app that composes the ORM some other way — and any test that wants the answer
/// without booting — can ask for the same verdict.
pub fn audit_soft_delete_bindings() -> anyhow::Result<()> {
    let pairs: Vec<_> = inventory::iter::<SoftDeleteRegistration>
        .into_iter()
        .map(|r| ((r.entity)(), (r.service)(), (r.tombstones)()))
        .collect();
    audit(&pairs)
}

/// `Err` naming every entity whose `soft_delete` flag no service backs.
///
/// Split from the hook so it is testable without a link-time registry: the hook
/// only resolves the fn pointers.
fn audit(pairs: &[(&str, &str, bool)]) -> anyhow::Result<()> {
    let unbound: Vec<String> = pairs
        .iter()
        .filter(|(_, _, tombstones)| !tombstones)
        .map(|(entity, service, _)| {
            format!(
                "`{entity}` is `#[expose(..., soft_delete)]` but `{service}` does not override \
                 `CrudService::soft_delete_column`: DELETE would erase the row for good and reads \
                 would never filter `deleted_at`. Add `fn soft_delete_column() -> \
                 Option<Column> {{ Some(Column::DeletedAt) }}` to the service, or drop \
                 `soft_delete` from the entity"
            )
        })
        .collect();
    if unbound.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "soft delete is declared on the entity but not on the service:\n  - {}",
        unbound.join("\n  - ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_halves_present_passes() {
        audit(&[("post", "features::posts::PostService", true)]).expect("both halves declared");
    }

    #[test]
    fn no_soft_delete_entity_at_all_passes() {
        audit(&[]).expect("an app with no tombstone column has nothing to check");
    }

    #[test]
    fn a_service_without_the_override_fails_boot_naming_both_halves() {
        // The regression this exists for: `nestrs g resource` scaffolds both
        // halves, so the trap only closes on someone editing the service — and
        // the symptom is a `204` with the row gone.
        let err = audit(&[("post", "features::posts::PostService", false)])
            .expect_err("a tombstone column no service writes must not reach a DELETE");
        let text = err.to_string();
        assert!(text.contains("post"), "names the entity: {text}");
        assert!(
            text.contains("features::posts::PostService"),
            "names the service to edit: {text}",
        );
        assert!(
            text.contains("soft_delete_column"),
            "names the override to add: {text}",
        );
        assert!(
            text.contains("drop `soft_delete`"),
            "names the other way out — a resource that really erases rows: {text}",
        );
    }

    #[test]
    fn every_unbound_entity_is_reported_at_once() {
        // One boot, one list: fixing them one refusal at a time is how a
        // migration of several resources turns into several rebuilds.
        let err = audit(&[
            ("post", "PostService", false),
            ("comment", "CommentService", true),
            ("tag", "TagService", false),
        ])
        .expect_err("two unbound entities");
        let text = err.to_string();
        assert!(text.contains("post") && text.contains("tag"), "{text}");
        assert!(
            !text.contains("comment"),
            "a bound entity is not reported: {text}"
        );
    }
}
