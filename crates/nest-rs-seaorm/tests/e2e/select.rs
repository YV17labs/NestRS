//! `src/select.rs` against Postgres: no condition a select's `filter` takes reads a
//! row past the ability's scope, whatever text it writes. Text closing what holds it
//! is refused before any statement; text holding an `OR` precedence would read past
//! the scope stays one operand beside it; a fragment closing what it opens runs.

use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{Ability, AbilityBuilder, Action};
use nest_rs_database::__private::with_request_executor;
use nest_rs_seaorm::__private::SeaOrmExecutor;
use nest_rs_seaorm::{Repo, RepoError};
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::{Condition, Expr, ExprTrait, Iden};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, Set};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "select_probe_widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub tag: String,
        pub live: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Two rows under one tag of the test's own: the one the ability admits, and one it
/// does not.
async fn seeded(conn: &DatabaseConnection) -> (Uuid, Uuid) {
    crate::harness::setup_shared_table(
        conn,
        "select_probe_widgets",
        "CREATE TABLE IF NOT EXISTS select_probe_widgets (
            id UUID PRIMARY KEY,
            tag TEXT NOT NULL,
            live BOOLEAN NOT NULL
        );",
    )
    .await;
    let tag = Uuid::now_v7().to_string();
    let (admitted, other) = (Uuid::now_v7(), Uuid::now_v7());
    for id in [admitted, other] {
        widget::ActiveModel {
            id: Set(id),
            tag: Set(tag.clone()),
            live: Set(true),
        }
        .insert(conn)
        .await
        .expect("seed a widget");
    }
    (admitted, other)
}

fn reading(admitted: Uuid) -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(move |p| p.eq(widget::Column::Id, admitted));
    Arc::new(b.build().expect("valid test ability"))
}

/// What a read under `reading(admitted)` narrowed by `condition` answers.
async fn read(
    conn: &DatabaseConnection,
    admitted: Uuid,
    condition: impl Into<Condition> + Send,
) -> Result<Vec<Uuid>, RepoError> {
    let condition = condition.into();
    with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        install_ability(reading(admitted), async move {
            let rows = Repo::<widget::Entity>::scoped(Action::Read)
                .filter(condition)
                .all()
                .await?;
            Ok(rows.into_iter().map(|row| row.id).collect())
        }),
    )
    .await
}

fn refused(answer: &Result<Vec<Uuid>, RepoError>) -> bool {
    matches!(answer, Err(err) if err.to_string().contains("refused before any statement"))
}

fn enum_named(type_name: &str) -> sea_orm::sea_query::Value {
    sea_orm::sea_query::Value::from(sea_orm::sea_query::Enum {
        type_name: type_name.into(),
        value: "a".into(),
    })
}

/// Each reads every row at the base: `(1 = 0) AND (1 = 1) OR (1 = 1)`.
#[tokio::test]
#[expect(clippy::disallowed_methods, reason = "the fragments under test")]
async fn a_fragment_closing_what_holds_it_reads_nothing() {
    let conn = crate::harness::connect().await;
    let (admitted, _) = seeded(&conn).await;

    for fragment in ["true) OR (true", "true)) OR ((true", "true OR true --"] {
        let answer = read(&conn, admitted, Condition::all().add(Expr::cust(fragment))).await;
        assert!(refused(&answer), "{fragment}: {answer:?}");
    }
    let commenting = widget::Column::Tag.eq(enum_named(r#"text" OR true --"#));
    let answer = read(&conn, admitted, Condition::all().add(commenting)).await;
    assert!(refused(&answer), "{answer:?}");
}

/// Balanced text the scope's `AND` would bind tighter than: an enum's type name, a
/// hand-written `Iden`, each holding an `OR`.
#[tokio::test]
async fn a_balanced_text_holding_an_or_reads_within_the_scope() {
    struct Widening;

    impl Iden for Widening {
        fn quoted(&self) -> Cow<'static, str> {
            Cow::Borrowed(r#"live" OR true OR "live"#)
        }

        fn unquoted(&self) -> &str {
            "live"
        }
    }

    let conn = crate::harness::connect().await;
    let (admitted, _) = seeded(&conn).await;

    let typed = widget::Column::Tag.eq(enum_named(r#"text" OR true OR "live"#));
    let iden = Expr::col(Widening).is_not_null();
    for condition in [Condition::all().add(typed), Condition::all().add(iden)] {
        let rows = read(&conn, admitted, condition)
            .await
            .expect("the text stays one operand, and runs");
        assert_eq!(rows, [admitted]);
    }
}

#[tokio::test]
#[expect(clippy::disallowed_methods, reason = "the fragment under test")]
async fn a_fragment_closing_what_it_opens_runs_beside_the_scope() {
    let conn = crate::harness::connect().await;
    let (admitted, other) = seeded(&conn).await;

    let either = Expr::cust_with_values(
        r#""select_probe_widgets"."id" = $1 OR "select_probe_widgets"."id" = $2 OR lower('x)') = 'x)'"#,
        [admitted, other],
    );
    let rows = read(&conn, admitted, Condition::all().add(either))
        .await
        .expect("a fragment closing all it opens runs");
    assert_eq!(rows, [admitted], "the scope still holds it");
}
