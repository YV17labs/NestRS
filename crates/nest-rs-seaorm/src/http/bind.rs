use std::any::{TypeId, type_name};
use std::marker::PhantomData;
use std::ops::Deref;

use nest_rs_authz::{Ability, ActionMarker, with_ability};
use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, ResolvesProvider};
use nest_rs_http::__private::poem_bridge::error_from_poem;
use nest_rs_http::{HttpError, ProblemDetails, StatusCode};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::{Access, CrudService, ServiceError};

/// The loaded, authorized entity bound from a path id, through service `S`.
/// Declare as a handler parameter (`user: Bind<Read, UsersService>`); read the
/// model via [`Deref`] or own it with [`into_inner`](Bind::into_inner).
///
/// A non-v7 id answers 400, an absent row 404, a denied one 403 (existence is
/// not hidden). The service is part of the boot's access contract
/// ([`ResolvesProvider`]); a route that still reaches none — no ability guard,
/// no request scope, `S` offered by no module — answers an opaque `500` and
/// files what was missing at `error`.
pub struct Bind<A, S: CrudService>(<S::Entity as EntityTrait>::Model, PhantomData<fn() -> A>);

impl<A, S: CrudService> Bind<A, S> {
    /// Take ownership of the loaded, authorized model.
    pub fn into_inner(self) -> <S::Entity as EntityTrait>::Model {
        self.0
    }
}

impl<A, S: CrudService> Deref for Bind<A, S> {
    type Target = <S::Entity as EntityTrait>::Model;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<A, S: CrudService + 'static> ResolvesProvider for Bind<A, S> {
    fn provider() -> (TypeId, &'static str) {
        (TypeId::of::<S>(), type_name::<S>())
    }
}

/// Why no entity is handed over.
enum Unbound {
    /// The id is not a UUID v7: `400`.
    NotV7,
    /// What the route needs to load it is missing — filed at `error` where it
    /// was found: an opaque `500`.
    Unwired,
    /// The service failed reading the row.
    Failed(ServiceError),
    /// The row exists and the caller may not have it: `403`.
    Denied,
    /// No such row: `404`.
    Missing,
}

impl<A, S> Bind<A, S>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    /// The row `id` names, loaded through `S` under the request's ability.
    async fn load(id: Uuid) -> Result<Self, Unbound> {
        if id.get_version_num() != 7 {
            return Err(Unbound::NotV7);
        }
        let Some(ability) = nest_rs_core::ambient::<Ability>() else {
            tracing::error!(
                target: crate::target::ORM,
                provider = type_name::<S>(),
                "missing request `Ability` — is the ability guard applied to this route?",
            );
            return Err(Unbound::Unwired);
        };
        let Some(scope) = nest_rs_http::current_request_scope() else {
            tracing::error!(
                target: crate::target::ORM,
                provider = type_name::<S>(),
                "request scope not installed — the transport edge must wrap the route tree",
            );
            return Err(Unbound::Unwired);
        };
        let Some(service) = scope.get::<S>() else {
            tracing::error!(
                target: crate::target::ORM,
                provider = type_name::<S>(),
                "no provider for the bound service — add it to a module's providers",
            );
            return Err(Unbound::Unwired);
        };
        match with_ability(ability.clone(), service.access(A::ACTION, id)).await {
            Ok(Access::Found(model)) => Ok(Bind(model, PhantomData)),
            Ok(Access::Denied) => Err(Unbound::Denied),
            Ok(Access::Missing) => Err(Unbound::Missing),
            Err(err) => Err(Unbound::Failed(ServiceError::Db(err))),
        }
    }
}

/// What an [`Unbound::Unwired`] answers: the client learns nothing of the
/// wiring.
fn opaque() -> ProblemDetails {
    ProblemDetails::internal().with_detail(OPAQUE_CLIENT_MESSAGE)
}

impl Unbound {
    fn into_poem(self) -> poem::Error {
        match self {
            Self::NotV7 => {
                poem::Error::from_string(nest_rs_core::UUID_V7_REQUIRED, StatusCode::BAD_REQUEST)
            }
            Self::Unwired => poem::Error::from(opaque()),
            Self::Failed(err) => poem::Error::from(err),
            Self::Denied => poem::Error::from_status(StatusCode::FORBIDDEN),
            Self::Missing => poem::Error::from_status(StatusCode::NOT_FOUND),
        }
    }

    fn into_http(self) -> HttpError {
        match self {
            Self::NotV7 => ProblemDetails::bad_request()
                .with_detail(nest_rs_core::UUID_V7_REQUIRED)
                .into(),
            Self::Unwired => opaque().into(),
            // `ServiceError` answers through poem's `ResponseError` until the
            // HTTP edge retypes it.
            Self::Failed(err) => error_from_poem(poem::Error::from(err)),
            Self::Denied => HttpError::from_status(StatusCode::FORBIDDEN),
            Self::Missing => HttpError::from_status(StatusCode::NOT_FOUND),
        }
    }
}

impl<'a, A, S> poem::FromRequest<'a> for Bind<A, S>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    async fn from_request(
        req: &'a poem::Request,
        body: &mut poem::RequestBody,
    ) -> poem::Result<Self> {
        nest_rs_http::__private::MaskProbe::mark();
        let poem::web::Path(id) = poem::web::Path::<Uuid>::from_request(req, body).await?;
        Self::load(id).await.map_err(Unbound::into_poem)
    }
}

impl<'a, A, S> nest_rs_http::FromRequest<'a> for Bind<A, S>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    async fn from_request(
        req: &'a nest_rs_http::Request,
        body: &mut nest_rs_http::RequestBody,
    ) -> nest_rs_http::Result<Self> {
        nest_rs_http::__private::MaskProbe::mark();
        let nest_rs_http::Path(id) = nest_rs_http::Path::<Uuid>::from_request(req, body).await?;
        Self::load(id).await.map_err(Unbound::into_http)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_authz::Read;
    use nest_rs_core::__private::{ParamProbe, ParamProbeResolves as _};
    use nest_rs_testing::LogCapture;
    use sea_orm::DbErr;

    use super::*;

    mod note {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "notes")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    struct NotesService;

    impl CrudService for NotesService {
        type Entity = note::Entity;
    }

    /// The probe `#[routes]` asks per parameter, at the reference depth its
    /// expansion calls it.
    #[test]
    fn the_handler_probe_reads_the_service_bind_resolves() {
        let resolved = (&&ParamProbe::<Bind<Read, NotesService>>::new()).resolved_provider();
        assert_eq!(
            resolved.map(|(id, _)| id),
            Some(TypeId::of::<NotesService>())
        );
    }

    #[tokio::test]
    async fn a_route_with_no_ability_answers_an_opaque_500_and_files_it_at_error() {
        let logs = LogCapture::install();
        let Err(unbound) = Bind::<Read, NotesService>::load(Uuid::now_v7()).await else {
            panic!("no ability is ambient");
        };
        let response = unbound.into_http().into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response.into_body().into_string().await.expect("body");
        assert!(body.contains(OPAQUE_CLIENT_MESSAGE), "{body}");
        assert!(
            !body.contains("NotesService"),
            "the type stays off the wire: {body}"
        );
        assert!(!body.contains("Ability"), "{body}");

        let event = logs.expect_one(
            "nest_rs::orm",
            "missing request `Ability` — is the ability guard applied to this route?",
        );
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("provider")
                .is_some_and(|provider| provider.contains("NotesService")),
            "{:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn an_id_that_is_not_v7_is_refused_before_anything_is_read() {
        let Err(unbound) = Bind::<Read, NotesService>::load(Uuid::new_v4()).await else {
            panic!("a v4 id is refused");
        };
        let response = unbound.into_http().into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().into_string().await.expect("body");
        assert!(body.contains(nest_rs_core::UUID_V7_REQUIRED), "{body}");
    }

    #[tokio::test]
    async fn db_errors_map_to_an_opaque_500_with_no_driver_text() {
        for answered in [
            nest_rs_http::__private::poem_bridge::response_from_poem(
                Unbound::Failed(ServiceError::Db(DbErr::Custom(
                    "connection to db.internal:5432 refused".into(),
                )))
                .into_poem()
                .into_response(),
            ),
            Unbound::Failed(ServiceError::Db(DbErr::Custom(
                "connection to db.internal:5432 refused".into(),
            )))
            .into_http()
            .into_response(),
        ] {
            assert_eq!(answered.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = answered.into_body().into_string().await.expect("body");
            assert!(
                !body.contains("db.internal"),
                "driver text must not reach the client: {body}"
            );
        }
    }
}
