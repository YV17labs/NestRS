//! The class gate `#[authorize(Action, Entity)]` declares, written once for
//! every edge — authentication first, then the class grant and the denial that
//! holds for every row, then the refusal a wider token would have fixed — and
//! the one `warn` every authorization refusal files.

use std::any::TypeId;

use nest_rs_core::{Edge, Failure, Posture, ServedAddress, UnitContext};
use nest_rs_guards::__private::UNAUTHENTICATED;
use nest_rs_guards::Denial;

use crate::{Ability, Action, ActionMarker, Subject};

/// What a refused caller is told when its principal holds no grant on the
/// class, or a denial no `when` narrows bars it.
const FORBIDDEN: &str = "forbidden";

/// What a refused caller is told when its token is too narrow: RFC 6750 §3.1's
/// description of `insufficient_scope`, one sentence on every edge.
pub(crate) const INSUFFICIENT_SCOPE: &str =
    "the request requires higher privileges than provided by the access token";

/// What a unit no ability reached is refused with, opaquely: a wiring defect,
/// one sentence wherever the gate runs.
pub(crate) const NO_AMBIENT_ABILITY: &str = "no ability reached a unit its posture gates";

/// The machine-readable denial reasons, spelled once.
pub(crate) mod reason {
    /// No principal at all — the gate's first rung.
    pub(crate) use nest_rs_guards::__private::ANONYMOUS_CALLER;
    /// A principal with no grant on the subject class.
    pub(crate) const NO_CLASS_GRANT: &str = "no_class_grant";
    /// A principal a denial no `when` narrows bars from the subject class,
    /// whatever it grants.
    pub(crate) const CLASS_DENIED: &str = "class_denied";
    /// A principal whose token is too narrow — RFC 6750 §3.1.
    pub(crate) const INSUFFICIENT_SCOPE: &str = "insufficient_scope";
    /// Nothing installed an ability: a wiring failure, reported by every
    /// fail-closed exit, masking included (via [`mask_reason`](crate::ability::mask_reason)).
    pub(crate) const NO_AMBIENT_ABILITY: &str = crate::ability::mask_reason::NO_AMBIENT_ABILITY;
    /// The ability guard's rules would not build — the request is refused
    /// rather than given an ability whose denial evaporates.
    #[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
    pub(crate) const RULES_MISCONFIGURED: &str = "rules_misconfigured";
    /// A field grant stripped a key the answer cannot be delivered without.
    /// Only the edges handing a typed value back reach it; HTTP and WS drop the key.
    #[cfg(any(feature = "graphql", feature = "mcp"))]
    pub(crate) const FIELD_NOT_GRANTED: &str = "field_not_granted";
}

/// Everything the one `authorization denied` event can carry, and the only
/// place any of its fields is named.
#[derive(Clone, Copy)]
pub(crate) struct Refusal<'a> {
    /// The edge that refused — its [`Edge`] word.
    pub transport: &'static str,
    /// Where the unit that refused is served, in the fields its `mounted` line
    /// names it by, where a unit names one.
    pub served: ServedAddress<'a>,
    /// The site that refused, by its host and method, where a unit names one.
    pub site: Option<(&'static str, &'static str)>,
    /// The action `#[authorize]` declared, where a posture declared one.
    pub action: Option<Action>,
    /// The subject class reached for.
    pub subject: Option<&'static str>,
    /// The wire keys a field grant stripped, joined — `tracing` records
    /// scalars, so the list is flattened here and kept structured on the wire.
    pub fields: Option<&'a str>,
    /// The machine-readable reason, from [`reason`].
    pub reason: Option<&'static str>,
    /// What the developer would change — prose, never the `reason` value.
    pub remedy: Option<&'static str>,
}

impl<'a> Refusal<'a> {
    /// A refusal on `transport`, with nothing else stated yet.
    pub(crate) fn on(transport: &'static str) -> Self {
        Self {
            transport,
            served: ServedAddress::default(),
            site: None,
            action: None,
            subject: None,
            fields: None,
            reason: None,
            remedy: None,
        }
    }

    /// A refusal on the unit `cx` describes: its edge, and its coordinates.
    pub(crate) fn of_unit(cx: &'a UnitContext<'_>) -> Self {
        let handler = cx.handler();
        Self {
            served: cx.served_address().unwrap_or_default(),
            site: Some((handler.host_name(), handler.method())),
            ..Self::on(cx.edge().map_or("unit", Edge::as_str))
        }
    }

    /// A refusal a mask makes on `unit`, whose edge states the transport.
    #[cfg(any(feature = "graphql", feature = "mcp"))]
    pub(crate) fn of_mask(unit: crate::wire_mask::MaskedUnit<'a>) -> Self {
        Self {
            served: unit.served,
            site: unit.site,
            ..Self::on(unit.transport.unwrap_or("unit"))
        }
    }

    /// The same refusal, naming what `#[authorize(A, S)]` declared.
    pub(crate) fn declaring<A: ActionMarker, S: Subject>(self) -> Self {
        Self {
            action: Some(A::ACTION),
            subject: Some(std::any::type_name::<S>()),
            ..self
        }
    }
}

/// The one `warn` every authorization refusal passes through, so a denial
/// cannot reach a client without leaving the queryable trace an incident is
/// answered from.
pub(crate) fn warn_denied(refusal: Refusal<'_>) {
    let Refusal {
        transport,
        served,
        site,
        action,
        subject,
        fields,
        reason,
        remedy,
    } = refusal;
    tracing::warn!(
        target: crate::TARGET,
        transport,
        host = site.map(|(host, _)| host),
        method = site.map(|(_, method)| method),
        verb = served.verb,
        path = served.path,
        kind = served.kind,
        field = served.field,
        event = served.event,
        tool = served.tool,
        prompt = served.prompt,
        resource = served.resource,
        // `Option<Action>` is not a `tracing` value; `field::debug` lets it be absent.
        action = action.map(tracing::field::debug),
        subject,
        fields,
        reason,
        remedy,
        "authorization denied",
    );
}

/// The one `error` a unit no ability reached files: a wiring defect, never the
/// caller's doing, so it is the operator's line and the client reads the
/// opaque answer.
pub(crate) fn error_unreached(refusal: Refusal<'_>) {
    let Refusal {
        transport,
        served,
        site,
        action,
        subject,
        remedy,
        ..
    } = refusal;
    tracing::error!(
        target: crate::TARGET,
        transport,
        host = site.map(|(host, _)| host),
        method = site.map(|(_, method)| method),
        verb = served.verb,
        path = served.path,
        kind = served.kind,
        field = served.field,
        event = served.event,
        tool = served.tool,
        prompt = served.prompt,
        resource = served.resource,
        action = action.map(tracing::field::debug),
        subject,
        reason = reason::NO_AMBIENT_ABILITY,
        remedy,
        "authorization denied",
    );
}

/// The class gate `#[authorize(A, S)]` declares, the same function on every
/// edge: whether the ability the unit carries grants action `A` on the subject
/// class `S`, every concrete action where `A` is [`Manage`](crate::Manage).
///
/// It reads the ability the site's guards attached, whether or not their stage
/// has made it ambient yet. A site that is not `#[public]` refuses a caller no
/// principal backs, whatever the visitor's rules grant: a grant written for
/// public reads never satisfies an `#[authorize]`, and a `#[public(A, S)]` naming
/// an entity licenses a visitor's reads for the unit. A denial no `when` narrows
/// matches every row, so it refuses here, whatever else the ability grants. A
/// refusal files one `warn` and comes back as the [`Denial`] the edge renders —
/// `401`, `403`, or `403` naming the scopes a wider token would carry
/// (RFC 6750 §3.1), never for a denial, which no scope lifts. A unit no
/// ability reached is a wiring defect: refused opaquely, filed at `error`.
///
/// It coerces to the pipeline's class gate, so a package's decorator emits it
/// for its own posture as the framework's do:
///
/// ```
/// use nest_rs_authz::{AbilityBuilder, Action, Read, class_gate};
/// # use std::any::TypeId;
/// # use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, declare_unit, new_unit_context};
/// # use nest_rs_core::{Handler, Posture, UnitView, operation_log::Kind};
/// # mod posts {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "posts")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # struct Jobs;
/// # struct JobView;
/// # impl UnitView for JobView {}
/// # static REVIEW: Handler = declare_handler(HandlerDeclaration {
/// #     unit: declare_unit("acme", "acme", Kind::Server, "acme.review"),
/// #     host: TypeId::of::<Jobs>(),
/// #     host_name: "Jobs",
/// #     method: "review",
/// #     address: "review",
/// #     posture: Posture::Gated,
/// #     meta: MetaLevels::default,
/// # });
/// # let mut view = JobView;
/// # let mut cx = new_unit_context(&REVIEW, &mut view, None, None);
/// # let mut reader = AbilityBuilder::new();
/// # reader.can(Action::Read, posts::Entity);
/// # cx.attach(nest_rs_authz::__private::AmbientAbility::new(reader.build()?));
///
/// let gate: nest_rs_core::__private::ClassGate = class_gate::<Read, posts::Entity>;
/// assert!(gate(&mut cx).is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn class_gate<A: ActionMarker, S: Subject>(cx: &mut UnitContext<'_>) -> Result<(), Failure> {
    let Some(ability) = crate::context::carried(cx) else {
        error_unreached(Refusal {
            remedy: Some(
                "bind the ability guard on this site's request — \
                 #[use_guards(AuthnGuard, AuthzGuard)], the app's global pool, or \
                 EndpointGuards for an in-band endpoint — and import AuthzModule",
            ),
            ..Refusal::of_unit(cx).declaring::<A, S>()
        });
        return Err(Denial::internal(NO_AMBIENT_ABILITY).into());
    };
    let posture = cx.reflector().posture();
    decide(&ability, A::ACTION, TypeId::of::<S>(), posture).map_err(|(why, denial)| {
        warn_denied(Refusal {
            reason: Some(why),
            ..Refusal::of_unit(cx).declaring::<A, S>()
        });
        Failure::from(denial)
    })?;
    if S::__ROWS && ability.is_visitor() {
        cx.attach(crate::context::VisitorReadNamed);
    }
    Ok(())
}

/// Whether `ability` grants `action` on the class `subject` at a site of
/// `posture`: the refusal's reason and its [`Denial`] otherwise.
pub(crate) fn decide(
    ability: &Ability,
    action: Action,
    subject: TypeId,
    posture: Posture,
) -> Result<(), (&'static str, Denial)> {
    if ability.is_visitor() && posture != Posture::Public {
        return Err((
            reason::ANONYMOUS_CALLER,
            Denial::unauthorized(UNAUTHENTICATED),
        ));
    }
    if ability.can_class(action, subject) {
        return Ok(());
    }
    if ability.denied_outright(action, subject) {
        return Err((reason::CLASS_DENIED, Denial::forbidden(FORBIDDEN)));
    }
    let missing = ability.missing_scopes(action, subject);
    if missing.is_empty() {
        return Err((reason::NO_CLASS_GRANT, Denial::forbidden(FORBIDDEN)));
    }
    Err((
        reason::INSUFFICIENT_SCOPE,
        Denial::insufficient_scope(missing, INSUFFICIENT_SCOPE),
    ))
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::{
        HandlerDeclaration, MetaLevels, declare_handler, declare_unit, new_unit_context,
    };
    use nest_rs_core::operation_log::{Kind, Unit};
    use nest_rs_core::{Handler, UnitView};
    use nest_rs_guards::Scope;
    use nest_rs_testing::LogCapture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{AbilityBuilder, Read, Update};

    mod post {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "posts")]
        pub(crate) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(crate) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    use post::Entity as Post;

    struct Jobs;
    struct JobView;

    impl UnitView for JobView {}

    const REVIEW: Unit = declare_unit("acme", "acme", Kind::Server, "acme.review");

    static GATED: Handler = declare_handler(HandlerDeclaration {
        unit: REVIEW,
        host: TypeId::of::<Jobs>(),
        host_name: "Jobs",
        method: "review",
        address: "review",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    static PUBLIC: Handler = declare_handler(HandlerDeclaration {
        unit: REVIEW,
        host: TypeId::of::<Jobs>(),
        host_name: "Jobs",
        method: "preview",
        address: "preview",
        posture: Posture::Public,
        meta: MetaLevels::default,
    });

    /// What the gate answers at `site` for a unit carrying `ability`, or none.
    fn gated(site: &'static Handler, ability: Option<Ability>) -> Result<(), Failure> {
        let mut view = JobView;
        let mut cx = new_unit_context(site, &mut view, None, None);
        if let Some(ability) = ability {
            cx.attach(crate::context::AmbientAbility::new(ability));
        }
        class_gate::<Read, Post>(&mut cx)
    }

    fn denial(refused: &Failure) -> &Denial {
        refused
            .find::<Denial>()
            .expect("a class gate refuses with a denial")
    }

    fn reader() -> Ability {
        let mut rules = AbilityBuilder::new();
        rules.can(Action::Read, Post);
        rules.build().expect("one grant builds")
    }

    fn visitor_reader() -> Ability {
        let mut rules = AbilityBuilder::new();
        rules.can(Action::Read, Post);
        rules.build_visitor().expect("one grant builds")
    }

    #[test]
    fn a_granted_ability_is_admitted_and_files_nothing() {
        let logs = LogCapture::install();
        assert!(gated(&GATED, Some(reader())).is_ok());
        logs.expect_none(crate::TARGET, "authorization denied");
    }

    /// How many times a view was asked where its unit is served.
    static ASKED: AtomicUsize = AtomicUsize::new(0);

    struct CountingView;

    impl UnitView for CountingView {
        fn served_address(&self) -> Option<ServedAddress<'_>> {
            ASKED.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// A unit's address is a refusal's to read: the admitted unit, the hot path,
    /// pays for no view call.
    #[test]
    fn only_a_refused_unit_is_asked_where_it_is_served() {
        let _logs = LogCapture::install();
        let mut view = CountingView;
        let mut cx = new_unit_context(&GATED, &mut view, None, None);
        cx.attach(crate::context::AmbientAbility::new(reader()));
        assert!(class_gate::<Read, Post>(&mut cx).is_ok());
        assert_eq!(ASKED.load(Ordering::Relaxed), 0, "admitted");

        assert!(class_gate::<Update, Post>(&mut cx).is_err());
        assert_eq!(ASKED.load(Ordering::Relaxed), 1, "refused");
    }

    #[test]
    fn an_anonymous_ability_is_refused_at_a_gated_site_whatever_its_grants() {
        let logs = LogCapture::install();
        let refused = gated(&GATED, Some(visitor_reader())).expect_err("no principal backs it");
        assert_eq!(denial(&refused).http_status(), 401);
        assert_eq!(denial(&refused).message(), UNAUTHENTICATED);
        let line = logs.expect_one(crate::TARGET, "authorization denied");
        assert_eq!(line.level, "warn");
        assert_eq!(
            line.field("reason").as_deref(),
            Some(reason::ANONYMOUS_CALLER)
        );
        assert_eq!(line.field("transport").as_deref(), Some("unit"));
    }

    #[test]
    fn a_public_site_reads_the_visitors_grants() {
        assert!(gated(&PUBLIC, Some(visitor_reader())).is_ok());
    }

    #[test]
    fn a_principal_without_the_grant_is_forbidden_once() {
        let logs = LogCapture::install();
        let empty = AbilityBuilder::new().build().expect("no rule builds");
        let refused = gated(&GATED, Some(empty)).expect_err("no grant");
        assert_eq!(denial(&refused).http_status(), 403);
        assert_eq!(denial(&refused).message(), FORBIDDEN);
        let line = logs.expect_one(crate::TARGET, "authorization denied");
        assert_eq!(
            line.field("reason").as_deref(),
            Some(reason::NO_CLASS_GRANT)
        );
        assert_eq!(
            line.field("subject").as_deref(),
            Some(std::any::type_name::<Post>())
        );
    }

    /// A grant the token would unlock beside a denial no `when` narrows: the
    /// denial decides, and no scope is advertised that would not change it.
    #[test]
    fn a_denial_no_when_narrows_is_forbidden_plainly_whatever_the_token_lacks() {
        let logs = LogCapture::install();
        let mut rules = AbilityBuilder::new().with_granted_scopes(Some(Vec::new().into()));
        rules
            .can(Action::Read, Post)
            .requires_scope(Scope::new("posts:read"));
        rules
            .can(Action::Manage, Post)
            .when(|p| p.eq(post::Column::Id, 1));
        rules.cannot(Action::Read, Post);
        let refused = gated(&GATED, Some(rules.build().expect("the rules build")))
            .expect_err("the denial holds for every row");

        assert_eq!(denial(&refused).http_status(), 403);
        assert_eq!(denial(&refused).message(), FORBIDDEN);
        assert_eq!(denial(&refused).bearer_error(), None);
        assert!(
            denial(&refused).required_scopes().is_empty(),
            "no challenge names a scope that would be refused all the same",
        );
        let line = logs.expect_one(crate::TARGET, "authorization denied");
        assert_eq!(line.field("reason").as_deref(), Some(reason::CLASS_DENIED));
    }

    #[test]
    fn a_unit_no_ability_reached_is_refused_opaquely_at_error() {
        let logs = LogCapture::install();
        let refused = gated(&GATED, None).expect_err("fails closed");
        assert_eq!(denial(&refused).http_status(), 500);
        let line = logs.expect_one(crate::TARGET, "authorization denied");
        assert_eq!(line.level, "error");
        assert_eq!(
            line.field("reason").as_deref(),
            Some(reason::NO_AMBIENT_ABILITY)
        );
    }
}
