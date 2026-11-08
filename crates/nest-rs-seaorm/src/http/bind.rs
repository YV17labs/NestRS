use nest_rs_authz::ActionMarker;
use nest_rs_http::{HttpError, ProblemDetails, StatusCode};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::bind::{Unbound, load};
use crate::{Authorized, CrudService, ServiceError};

/// The subject `#[authorize(A, bind = S)]` loads for a route: the row its
/// path's id names, through service `S` under the request's ability, handed to
/// the handler's `Authorized<A, E>` parameter.
///
/// A non-v7 id answers 400, an absent row 404, a denied one 403 (existence is
/// not hidden). The decorator puts `S` in the boot's access contract, and the
/// boot refuses a route no ability guard reaches; what the unit still lacks at
/// request time — an ability, its request scope — answers an opaque `500` and
/// files at `error`.
pub async fn http_bind<A, S>(
    req: &nest_rs_http::Request,
    body: &mut nest_rs_http::RequestBody,
) -> nest_rs_http::Result<Authorized<A, S::Entity>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    let nest_rs_http::Path(id) =
        <nest_rs_http::Path<Uuid> as nest_rs_http::FromRequest>::from_request(req, body).await?;
    load::<A, S>(id).await.map_err(into_http)
}

/// What a route answers for the subject it could not load.
fn into_http(unbound: Unbound) -> HttpError {
    match unbound {
        Unbound::NotV7 => ProblemDetails::bad_request()
            .with_detail(nest_rs_core::UUID_V7_REQUIRED)
            .into(),
        Unbound::Unwired { why, provider } => {
            tracing::error!(
                target: crate::target::ORM,
                provider,
                error = %why,
                "a bound subject cannot be loaded",
            );
            ProblemDetails::internal().into()
        }
        Unbound::Failed(err) => HttpError::from(ServiceError::Db(err)),
        Unbound::Denied => HttpError::from_status(StatusCode::FORBIDDEN),
        Unbound::Missing => HttpError::from_status(StatusCode::NOT_FOUND),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_authz::Read;
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

    #[tokio::test]
    async fn a_route_with_no_ability_answers_an_opaque_500_and_files_it_at_error() {
        let logs = LogCapture::install();
        let Err(unbound) = load::<Read, NotesService>(Uuid::now_v7()).await else {
            panic!("no ability is ambient");
        };
        let response = into_http(unbound).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response.into_body().into_string().await.expect("body");
        assert_eq!(document(&body), internal_document().await, "{body}");
        assert!(
            !body.contains("NotesService"),
            "the type stays off the wire: {body}"
        );
        assert!(!body.contains("Ability"), "{body}");

        let event = logs.expect_one("nest_rs::orm", "a bound subject cannot be loaded");
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("error")
                .is_some_and(|why| why.contains("ability guard")),
            "{:?}",
            event.fields,
        );
        assert_eq!(
            event.field("provider").as_deref(),
            Some("NotesService"),
            "{:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn an_id_that_is_not_v7_is_refused_before_anything_is_read() {
        let Err(unbound) = load::<Read, NotesService>(Uuid::new_v4()).await else {
            panic!("a v4 id is refused");
        };
        let response = into_http(unbound).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().into_string().await.expect("body");
        assert!(body.contains(nest_rs_core::UUID_V7_REQUIRED), "{body}");
    }

    /// A refusal answers its class's document, the code `#[crud]`'s service
    /// error and the in-band edges name it by.
    #[tokio::test]
    async fn a_refusal_answers_its_class_s_document() {
        use nest_rs_core::{Problem, code};

        for (unbound, class) in [
            (Unbound::Denied, Problem::new(403, code::FORBIDDEN)),
            (Unbound::Missing, Problem::new(404, code::NOT_FOUND)),
            (
                Unbound::NotV7,
                Problem::new(400, code::INVALID_ARGUMENT)
                    .with_detail(nest_rs_core::UUID_V7_REQUIRED),
            ),
        ] {
            let response = into_http(unbound).into_response();
            assert_eq!(response.status().as_u16(), class.status());
            let body = response.into_body().into_string().await.expect("body");
            assert_eq!(
                document(&body),
                serde_json::to_value(ProblemDetails::from_problem(&class)).expect("a document"),
                "{body}",
            );
        }
    }

    #[tokio::test]
    async fn db_errors_map_to_an_opaque_500_with_no_driver_text() {
        let answered = into_http(Unbound::Failed(DbErr::Custom(
            "connection to db.internal:5432 refused".into(),
        )))
        .into_response();
        assert_eq!(answered.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = answered.into_body().into_string().await.expect("body");
        assert!(
            !body.contains("db.internal"),
            "driver text must not reach the client: {body}"
        );
        assert_eq!(document(&body), internal_document().await, "{body}");
    }

    /// The one document every opaque `500` is on HTTP, whoever answered it:
    /// what `nest_rs_http::Opaque` answers.
    async fn internal_document() -> serde_json::Value {
        use nest_rs_http::Opaque as _;

        let opaque = Err::<(), _>(std::io::Error::other("unwired"))
            .opaque()
            .expect_err("a failure")
            .into_response();
        let body = opaque.into_body().into_string().await.expect("body");
        let said = document(&body);
        assert_eq!(said["code"], "INTERNAL", "{said}");
        said
    }

    fn document(body: &str) -> serde_json::Value {
        serde_json::from_str(body).expect("problem json")
    }
}
