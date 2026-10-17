//! The class gate `#[authorize(Action, Entity)]` declares, written once for
//! every edge — authentication first, then the class grant, then the refusal a
//! wider token would have fixed — and the one `warn` every authorization
//! refusal files.

use std::any::TypeId;

use nest_rs_core::{Edge, Failure, Posture, UnitContext};
use nest_rs_guards::Denial;

use crate::{Ability, Action, ActionMarker, Subject};

/// What a refused caller is told when no principal backs the unit.
const UNAUTHENTICATED: &str = "unauthenticated";

/// What a refused caller is told when its principal holds no grant on the class.
const FORBIDDEN: &str = "forbidden";

/// What a refused caller is told when its token is too narrow: RFC 6750 §3.1's
/// description of `insufficient_scope`, one sentence on every edge.
pub(crate) const INSUFFICIENT_SCOPE: &str =
    "the request requires higher privileges than provided by the access token";

/// The machine-readable denial reasons, spelled once.
pub(crate) mod reason {
    /// No principal at all — the gate's first rung.
    pub(crate) const ANONYMOUS_CALLER: &str = "anonymous_caller";
    /// A principal with no grant on the subject class.
    pub(crate) const NO_CLASS_GRANT: &str = "no_class_grant";
    /// A principal whose token is too narrow — RFC 6750 §3.1.
    pub(crate) const INSUFFICIENT_SCOPE: &str = "insufficient_scope";
    /// Nothing installed an ability: a wiring failure, reported by every
    /// fail-closed exit, masking included (via [`mask_reason`](crate::ability::mask_reason)).
    pub(crate) const NO_AMBIENT_ABILITY: &str = crate::ability::mask_reason::NO_AMBIENT_ABILITY;
    /// A field grant stripped a key the answer cannot be delivered without.
    /// Only the edges handing a typed value back reach it; HTTP and WS drop the key.
    #[cfg(any(feature = "graphql", feature = "mcp"))]
    pub(crate) const FIELD_NOT_GRANTED: &str = "field_not_granted";
}

/// The edge a mask's refusal was filed on, spelled once per edge.
pub(crate) mod transport {
    /// The HTTP edge — a route's response mask.
    #[cfg(feature = "http")]
    pub(crate) const HTTP: &str = "http";
    /// The GraphQL edge — a resolver operation or a federation root field.
    #[cfg(feature = "graphql")]
    pub(crate) const GRAPHQL: &str = "graphql";
    /// The WebSocket edge — one message on an established connection.
    #[cfg(feature = "ws")]
    pub(crate) const WS: &str = "ws";
    /// The MCP edge — one tool call or prompt fetch.
    #[cfg(feature = "mcp")]
    pub(crate) const MCP: &str = "mcp";
}

/// Everything the one `authorization denied` event can carry, and the only
/// place any of its fields is named.
#[derive(Clone, Copy)]
pub(crate) struct Refusal<'a> {
    /// The edge that refused — its [`Edge`] word.
    pub transport: &'static str,
    /// The edge's own name for the unit of work, where it has one beside the
    /// subject: a WS event name. GraphQL and MCP report the operation on the
    /// chain's own line instead.
    pub event: Option<&'a str>,
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
            event: None,
            site: None,
            action: None,
            subject: None,
            fields: None,
            reason: None,
            remedy: None,
        }
    }

    /// A refusal on the unit `cx` describes: its edge, and its event on WS.
    pub(crate) fn of_unit(cx: &'a UnitContext<'_>) -> Self {
        let edge = cx.edge();
        let handler = cx.handler();
        Self {
            event: (edge == Some(Edge::Ws)).then(|| handler.address()),
            site: Some((handler.host_name(), handler.method())),
            ..Self::on(edge.map_or("unit", Edge::as_str))
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
        event,
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
        event,
        host = site.map(|(host, _)| host),
        method = site.map(|(_, method)| method),
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
        event,
        site,
        action,
        subject,
        remedy,
        ..
    } = refusal;
    tracing::error!(
        target: crate::TARGET,
        transport,
        event,
        host = site.map(|(host, _)| host),
        method = site.map(|(_, method)| method),
        action = action.map(tracing::field::debug),
        subject,
        reason = reason::NO_AMBIENT_ABILITY,
        remedy,
        "authorization denied",
    );
}

/// The class gate `#[authorize(A, S)]` declares, the same function on every
/// edge: whether the ability the unit carries grants action `A` on the subject
/// class `S`.
///
/// It reads the ability the site's guards attached, whether or not their stage
/// has made it ambient yet. A site that is not `#[public]` refuses a caller no
/// principal backs, whatever the visitor's rules grant: a grant written for
/// public reads never satisfies an `#[authorize]`. A refusal files one `warn`
/// and comes back as the [`Denial`] the edge renders — `401`, `403`, or `403`
/// naming the scopes a wider token would carry (RFC 6750 §3.1). A unit no
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
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
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
/// # cx.attach(reader.build()?);
///
/// let gate: nest_rs_core::__private::ClassGate = class_gate::<Read, posts::Entity>;
/// assert!(gate(&mut cx).is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn class_gate<A: ActionMarker, S: Subject>(cx: &mut UnitContext<'_>) -> Result<(), Failure> {
    let refusal = Refusal::of_unit(cx).declaring::<A, S>();
    let Some(ability) = cx.ambient::<Ability>() else {
        error_unreached(Refusal {
            remedy: Some(
                "bind the ability guard on this site's request — \
                 #[use_guards(AuthnGuard, AuthzGuard)], the app's global pool, or \
                 EndpointGuards for an in-band endpoint — and import AuthzModule",
            ),
            ..refusal
        });
        return Err(Denial::internal("no ability reached a unit its posture gates").into());
    };
    let posture = cx.reflector().posture();
    decide(&ability, A::ACTION, TypeId::of::<S>(), posture).map_err(|(why, denial)| {
        warn_denied(Refusal {
            reason: Some(why),
            ..refusal
        });
        Failure::from(denial)
    })
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
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::{AbilityBuilder, Read};

    mod post {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
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
            cx.attach(ability);
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
