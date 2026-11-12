//! The wire type of `#[expose]` (`src/dto.rs` in the macro crate): every
//! exposed entity names it through `Exposed`, on every build, `graphql` or not,
//! and each row it is built from is judged there, under the ability installed.

use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{AbilityBuilder, Action};
use nest_rs_seaorm::{Exposed, expose};
use sea_orm::entity::prelude::*;
use serde_json::json;

mod note {
    use super::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub body: String,
        pub draft: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A ledger whose `headline` the table calls `display_title`, a raw-identifier
/// column, a timestamp and an id a grant may withhold, and hidden columns.
mod ledger {
    use super::*;

    #[expose(name = "Ledger")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "ledgers")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub r#type: String,
        #[sea_orm(column_name = "display_title")]
        #[expose(nullable)]
        pub headline: String,
        #[expose(nullable)]
        pub owner_id: Uuid,
        #[expose(nullable)]
        pub closed_at: DateTimeWithTimeZone,
        #[expose]
        pub note: Option<String>,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) fn row() -> Model {
        Model {
            id: Uuid::nil(),
            r#type: "cash".into(),
            headline: "Q3".into(),
            owner_id: Uuid::nil(),
            closed_at: DateTimeWithTimeZone::parse_from_rfc3339("2026-10-08T12:30:00.250+00:00")
                .expect("a timestamp"),
            note: Some("kept".into()),
            secret: "hidden".into(),
        }
    }
}

/// The wire-only arm: `#[wire_enum]`'s enum as a column of an entity with no
/// GraphQL object.
mod release {
    use nest_rs_core::wire_enum;

    use super::*;

    #[wire_enum]
    #[derive(EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    #[serde(rename_all = "lowercase")]
    pub enum Stage {
        #[sea_orm(string_value = "draft")]
        Draft,
        #[sea_orm(string_value = "shipped")]
        Shipped,
    }

    #[expose(name = "Release")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "releases")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub stage: Stage,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A docket whose judge, filing date and note a grant may withhold, on every edge.
#[cfg(feature = "graphql")]
mod docket {
    use super::*;

    #[expose(name = "Docket", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "dockets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose(nullable)]
        pub judge: String,
        #[expose(nullable)]
        pub filed_at: DateTimeWithTimeZone,
        #[expose]
        pub note: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// `f` run under an ability `build` grants.
async fn under<T>(build: impl FnOnce(&mut AbilityBuilder), f: impl FnOnce() -> T) -> T {
    let mut ab = AbilityBuilder::new();
    build(&mut ab);
    let ability = Arc::new(ab.build().expect("a valid ability"));
    install_ability(ability, async move { f() }).await
}

#[test]
fn an_exposed_entity_names_the_wire_type_it_generated() {
    let row = note::Model {
        id: Uuid::nil(),
        body: "hello".into(),
        draft: true,
    };
    let wire: <note::Entity as Exposed>::Wire = From::from(&row);
    let named: note::Note = wire;
    assert_eq!(named.body, "hello");
}

/// Every key is written, a withheld one as `null`; a hidden column has no key;
/// an id and a timestamp keep chrono's and uuid's own spellings.
#[tokio::test]
async fn a_row_writes_every_exposed_key_and_null_where_its_grant_withholds() {
    let whole = under(
        |ab| {
            ab.can(Action::Read, ledger::Entity);
        },
        || serde_json::to_value(ledger::Ledger::from(&ledger::row())).expect("serializes"),
    )
    .await;
    assert_eq!(
        whole,
        json!({
            "id": Uuid::nil(),
            "type": "cash",
            "headline": "Q3",
            "owner_id": Uuid::nil(),
            "closed_at": "2026-10-08T12:30:00.250Z",
            "note": "kept",
        }),
    );

    let narrowed = under(
        |ab| {
            ab.can(Action::Read, ledger::Entity)
                .fields([ledger::Column::Id, ledger::Column::Type]);
        },
        || serde_json::to_value(ledger::Ledger::from(&ledger::row())).expect("serializes"),
    )
    .await;
    assert_eq!(
        narrowed,
        json!({
            "id": Uuid::nil(),
            "type": "cash",
            "headline": null,
            "owner_id": null,
            "closed_at": null,
            "note": null,
        }),
    );
}

/// `.fields` names a column, not a spelling of it: `column_name` moves the
/// database's name, and the grant still keeps what the wire calls that column.
#[tokio::test]
async fn a_field_grant_on_a_renamed_column_keeps_its_wire_key() {
    let row = under(
        |ab| {
            ab.can(Action::Read, ledger::Entity).fields([
                ledger::Column::Id,
                ledger::Column::Type,
                ledger::Column::Headline,
            ]);
        },
        || ledger::Ledger::from(&ledger::row()),
    )
    .await;
    assert_eq!(row.headline.as_deref(), Some("Q3"));
    assert_eq!(row.note, None);
}

/// A row a judgement built answers only where that judgement ran.
#[tokio::test]
async fn a_row_answers_no_wire_outside_the_unit_that_judged_it() {
    let row = under(
        |ab| {
            ab.can(Action::Read, ledger::Entity);
        },
        || ledger::Ledger::from(&ledger::row()),
    )
    .await;
    let refused = serde_json::to_value(&row).expect_err("judged for another caller");
    assert_eq!(refused.to_string(), "a row judged for another caller");
    let refused = serde_json::to_value(ledger::Ledger::from(&ledger::row()))
        .expect_err("judged by no ability");
    assert_eq!(refused.to_string(), "a row no ability judged");
}

/// The schema says what a reply holds: a column a grant may withhold is
/// required and nullable, its format kept; every other is required and not; a
/// hidden one is no property.
#[test]
fn the_schema_says_which_columns_a_grant_may_withhold() {
    let schema = nest_rs_seaorm::__private::schemars::schema_for!(ledger::Ledger);
    let json = serde_json::to_value(&schema).expect("the schema serializes");
    let properties = &json["properties"];
    assert_eq!(properties["id"]["type"], "string", "{json}");
    assert_eq!(properties["id"]["format"], "uuid", "{json}");
    assert_eq!(
        properties["headline"]["type"],
        json!(["string", "null"]),
        "{json}"
    );
    assert_eq!(
        properties["owner_id"]["type"],
        json!(["string", "null"]),
        "{json}"
    );
    assert_eq!(properties["owner_id"]["format"], "uuid", "{json}");
    assert_eq!(properties["closed_at"]["format"], "date-time", "{json}");
    assert!(properties.get("secret").is_none(), "{json}");
    let mut required: Vec<&str> = json["required"]
        .as_array()
        .expect("a required list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    required.sort_unstable();
    assert_eq!(
        required,
        ["closed_at", "headline", "id", "owner_id", "type"],
        "a nullable key is always written, an `Option` column's may be read absent: {json}",
    );
}

/// What each property of a JSON Schema object says it is, sorted by key.
#[cfg(feature = "graphql")]
fn descriptions(schema: &serde_json::Value) -> Vec<(String, Option<String>)> {
    let mut said: Vec<_> = schema["properties"]
        .as_object()
        .expect("an object's properties")
        .iter()
        .map(|(key, property)| {
            let description = property["description"].as_str().map(str::to_owned);
            (key.clone(), description)
        })
        .collect();
    said.sort();
    said
}

/// Each property a grant may withhold says so in every schema an edge publishes —
/// the JSON Schema OpenAPI's components and an MCP tool's output schema are, and
/// the GraphQL SDL — whatever answers the row, and no other property says it.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn each_column_a_grant_may_withhold_says_so_in_every_edge_s_schema() {
    use nest_rs_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};

    struct Query;

    #[Object]
    impl Query {
        async fn docket(&self) -> Option<docket::Docket> {
            None
        }
    }

    let withholdable = || Some(nest_rs_seaorm::__withholdable_doc!().to_owned());
    let said_of = |filed_at: &str| {
        let mut said = vec![
            ("id".to_owned(), None),
            ("title".to_owned(), None),
            ("judge".to_owned(), withholdable()),
            (filed_at.to_owned(), withholdable()),
            ("note".to_owned(), withholdable()),
        ];
        said.sort();
        said
    };
    let wire = said_of("filed_at");

    let json_schema = nest_rs_seaorm::__private::schemars::schema_for!(docket::Docket);
    let json_schema = serde_json::to_value(&json_schema).expect("the schema serializes");
    assert_eq!(descriptions(&json_schema), wire, "{json_schema}");

    let output = nest_rs_mcp::rmcp::handler::server::tool::schema_for_output::<docket::Docket>();
    let output = serde_json::Value::Object((*output).clone());
    assert_eq!(descriptions(&output), wire, "MCP: {output}");

    let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
    let introspected = schema
        .execute(r#"{ __type(name: "Docket") { fields { name description } } }"#)
        .await
        .into_result()
        .expect("introspection answers");
    let introspected = introspected.data.into_json().expect("the answer is JSON");
    let mut fields: Vec<(String, Option<String>)> = introspected["__type"]["fields"]
        .as_array()
        .expect("the object's fields")
        .iter()
        .map(|field| {
            let name = field["name"].as_str().expect("a field's name").to_owned();
            (name, field["description"].as_str().map(str::to_owned))
        })
        .collect();
    fields.sort();
    assert_eq!(fields, said_of("filedAt"), "GraphQL: {introspected}");
}

/// A raw identifier serializes without its `r#`, so its key does too.
#[tokio::test]
async fn a_raw_identifier_is_keyed_as_it_serializes() {
    let written = under(
        |ab| {
            ab.can(Action::Read, ledger::Entity);
        },
        || serde_json::to_string(&ledger::Ledger::from(&ledger::row())).expect("serializes"),
    )
    .await;
    assert!(written.contains(r#""type":"cash""#), "{written}");
    assert!(!written.contains("r#"), "{written}");
}

#[tokio::test]
async fn the_enum_column_reaches_the_wire_dto_verbatim() {
    let id = Uuid::now_v7();
    let written = under(
        |ab| {
            ab.can(Action::Read, release::Entity);
        },
        || {
            let wire = release::Release::from(&release::Model {
                id,
                stage: release::Stage::Shipped,
            });
            assert_eq!(wire.stage, release::Stage::Shipped);
            serde_json::to_value(&wire).expect("the wire DTO serializes")
        },
    )
    .await;
    assert_eq!(written["stage"], json!("shipped"));
}

/// A reply's grant may withhold only a column its wire type answers `null`.
#[test]
fn a_reply_grant_withholding_a_column_no_null_answers_fails_the_build() {
    let mut ab = AbilityBuilder::new();
    ab.can(Action::Read, ledger::Entity)
        .fields([ledger::Column::Id]);
    let Err(refused) = ab.build() else {
        panic!("`type` cannot answer null");
    };
    assert!(
        refused.to_string().contains("withholds `type`"),
        "{refused}"
    );
}
