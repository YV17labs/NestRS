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

/// A hearing: an instant in each shape a column holds one — bare, an `Option`,
/// one a grant may withhold, at UTC and at the server's offset, one a write
/// sets — and an id, bare and in an `Option`, each one a write sets too.
#[cfg(feature = "graphql")]
mod hearing {
    use super::*;

    #[expose(name = "Hearing", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "hearings")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub scheduled_at: DateTimeWithTimeZone,
        #[expose]
        pub held_at: Option<DateTimeWithTimeZone>,
        #[expose(nullable)]
        pub adjourned_at: DateTimeWithTimeZone,
        #[expose]
        pub recorded_at: DateTimeUtc,
        #[expose]
        pub noted_at: Option<DateTimeUtc>,
        #[expose]
        pub sealed_at: DateTimeLocal,
        #[expose(input(create))]
        pub court_id: Uuid,
        #[expose(input(create))]
        pub clerk_id: Option<Uuid>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    /// The instant each column of `row` holds, at UTC, as Postgres reads one back.
    pub(super) const AT: &str = "2026-10-08T12:30:00.250Z";

    /// The id each id column of `row` holds.
    pub(super) const COURT: u128 = 0x0192_0000_0000_7000_8000_0000_0000_01c1;

    pub(super) fn row() -> Model {
        let at = DateTimeWithTimeZone::parse_from_rfc3339(AT).expect("a timestamp");
        Model {
            id: Uuid::nil(),
            scheduled_at: at,
            held_at: Some(at),
            adjourned_at: at,
            recorded_at: at.to_utc(),
            noted_at: Some(at.to_utc()),
            sealed_at: at.into(),
            court_id: Uuid::from_u128(COURT),
            clerk_id: Some(Uuid::from_u128(COURT)),
        }
    }

    /// Every instant column, as the wire keys it and as GraphQL names it.
    pub(super) const INSTANTS: [(&str, &str); 6] = [
        ("scheduled_at", "scheduledAt"),
        ("held_at", "heldAt"),
        ("adjourned_at", "adjournedAt"),
        ("recorded_at", "recordedAt"),
        ("noted_at", "notedAt"),
        ("sealed_at", "sealedAt"),
    ];
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
fn descriptions(schema: &serde_json::Value) -> Vec<(&str, Option<&str>)> {
    let mut said: Vec<_> = schema["properties"]
        .as_object()
        .expect("an object's properties")
        .iter()
        .map(|(key, property)| (key.as_str(), property["description"].as_str()))
        .collect();
    said.sort_unstable();
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

    let withholdable = Some(nest_rs_seaorm::__withholdable_doc!());
    let said_of = |filed_at| {
        let mut said = vec![
            ("id", None),
            ("title", None),
            ("judge", withholdable),
            (filed_at, withholdable),
            ("note", withholdable),
        ];
        said.sort_unstable();
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
    let mut fields: Vec<_> = introspected["__type"]["fields"]
        .as_array()
        .expect("the object's fields")
        .iter()
        .map(|field| {
            let name = field["name"].as_str().expect("a field's name");
            (name, field["description"].as_str())
        })
        .collect();
    fields.sort_unstable();
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

/// Serves `Hearing` from its one root field, which builds the row where it
/// answers, beside the field a write's input would reach.
#[cfg(feature = "graphql")]
struct HearingQuery;

#[cfg(feature = "graphql")]
#[nest_rs_graphql::async_graphql::Object]
impl HearingQuery {
    async fn hearing(&self) -> hearing::Hearing {
        hearing::Hearing::from(&hearing::row())
    }

    async fn file(&self, input: hearing::CreateHearing) -> bool {
        let _ = input;
        true
    }
}

#[cfg(feature = "graphql")]
fn hearing_schema() -> nest_rs_graphql::async_graphql::Schema<
    HearingQuery,
    nest_rs_graphql::async_graphql::EmptyMutation,
    nest_rs_graphql::async_graphql::EmptySubscription,
> {
    use nest_rs_graphql::async_graphql::{EmptyMutation, EmptySubscription, Schema};

    Schema::new(HearingQuery, EmptyMutation, EmptySubscription)
}

/// `f` run under an ability reading every hearing.
#[cfg(feature = "graphql")]
async fn as_a_reader_of_hearings<T>(f: impl Future<Output = T>) -> T {
    let mut ab = AbilityBuilder::new();
    ab.can(Action::Read, hearing::Entity);
    install_ability(Arc::new(ab.build().expect("a valid ability")), f).await
}

/// What introspection says of an object's fields, or an input's: each named type,
/// and whether it is Non-Null.
#[cfg(feature = "graphql")]
fn typed(fields: &serde_json::Value) -> std::collections::HashMap<&str, (&str, bool)> {
    fields
        .as_array()
        .expect("the fields")
        .iter()
        .map(|field| {
            let ty = &field["type"];
            let typed = if ty["kind"] == "NON_NULL" {
                (ty["ofType"]["name"].as_str().expect("a named type"), true)
            } else {
                (ty["name"].as_str().expect("a named type"), false)
            };
            (field["name"].as_str().expect("a field's name"), typed)
        })
        .collect()
}

/// An instant is one GraphQL scalar, `DateTime`, whether its column is an `Option`,
/// one a grant may withhold or neither, whichever zone it holds — the scalar a
/// write's input takes — and a `date-time` in every schema the other edges publish.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn an_instant_is_one_graphql_scalar_and_a_date_time_in_every_schema() {
    let introspected = hearing_schema()
        .execute(
            r#"{
                read: __type(name: "Hearing") { fields { name type { kind name ofType { name } } } }
                write: __type(name: "CreateHearing") {
                    inputFields { name type { kind name ofType { name } } }
                }
            }"#,
        )
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    let read = typed(&introspected["read"]["fields"]);
    for (_, field) in hearing::INSTANTS {
        let non_null = matches!(field, "scheduledAt" | "recordedAt" | "sealedAt");
        assert_eq!(
            read[field],
            ("DateTime", non_null),
            "{field}: {introspected}"
        );
    }
    let write = typed(&introspected["write"]["inputFields"]);
    assert_eq!(write["scheduledAt"], ("DateTime", true), "{introspected}");

    let json_schema = nest_rs_seaorm::__private::schemars::schema_for!(hearing::Hearing);
    let json_schema = serde_json::to_value(&json_schema).expect("the schema serializes");
    let output = nest_rs_mcp::rmcp::handler::server::tool::schema_for_output::<hearing::Hearing>();
    let output = serde_json::Value::Object((*output).clone());
    for (key, _) in hearing::INSTANTS {
        assert_eq!(
            json_schema["properties"][key]["format"], "date-time",
            "{key}: {json_schema}"
        );
        assert_eq!(
            output["properties"][key]["format"], "date-time",
            "MCP {key}: {output}"
        );
    }
}

/// An instant is spelled alike on GraphQL and on every edge serde writes — a REST
/// body, a WebSocket message, an MCP tool's answer — `Z` at offset zero, whether its
/// column is an `Option` or not.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn an_instant_is_spelled_alike_on_graphql_and_on_every_edge_serde_writes() {
    let selection: Vec<&str> = hearing::INSTANTS.iter().map(|(_, field)| *field).collect();
    let query = format!("{{ hearing {{ {} }} }}", selection.join(" "));
    let schema = hearing_schema();
    let (answered, serialized) = as_a_reader_of_hearings(async {
        let answered = schema
            .execute(query.as_str())
            .await
            .into_result()
            .expect("the query answers")
            .data
            .into_json()
            .expect("the answer is JSON");
        let serialized = serde_json::to_value(hearing::Hearing::from(&hearing::row()))
            .expect("the row serializes");
        (answered, serialized)
    })
    .await;
    for (key, field) in hearing::INSTANTS {
        assert_eq!(
            answered["hearing"][field], serialized[key],
            "{field}: {answered} where serde wrote {serialized}",
        );
        if key != "sealed_at" {
            assert_eq!(serialized[key], hearing::AT, "{key}: {serialized}");
        }
    }
}

/// The `DateTime` a wire row registers says what async-graphql's own says, the one
/// a write's input registers, so a schema reads alike whichever comes first.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn the_date_time_a_row_registers_is_described_as_the_one_an_input_registers() {
    use nest_rs_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};

    struct Chrono;

    #[Object]
    impl Chrono {
        async fn at(&self) -> DateTimeWithTimeZone {
            DateTimeWithTimeZone::default()
        }
    }

    const DESCRIBED: &str = r#"{ __type(name: "DateTime") { kind description specifiedByURL } }"#;
    let ours = hearing_schema()
        .execute(DESCRIBED)
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    let theirs = Schema::new(Chrono, EmptyMutation, EmptySubscription)
        .execute(DESCRIBED)
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    assert_eq!(ours, theirs);
    assert_eq!(
        ours["__type"]["specifiedByURL"], "https://datatracker.ietf.org/doc/html/rfc3339",
        "{ours}",
    );
}

/// The `ID` a wire row registers is async-graphql's own, as a `#[crud]` id argument
/// registers it, so a schema reads alike whichever comes first.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn the_id_a_row_registers_is_described_as_async_graphql_s_own() {
    use nest_rs_graphql::async_graphql::{EmptyMutation, EmptySubscription, ID, Object, Schema};

    struct Plain;

    #[Object]
    impl Plain {
        async fn plain(&self, id: ID) -> ID {
            id
        }
    }

    const DESCRIBED: &str = r#"{ __type(name: "ID") { kind description specifiedByURL } }"#;
    let ours = hearing_schema()
        .execute(DESCRIBED)
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    let theirs = Schema::new(Plain, EmptyMutation, EmptySubscription)
        .execute(DESCRIBED)
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    assert_eq!(ours, theirs);
    assert_eq!(ours["__type"]["kind"], "SCALAR", "{ours}");
}

/// An id is one GraphQL type, `ID`, whether its column is an `Option` or not and
/// whether a reply holds it or a write's input carries it — the type a `#[crud]`
/// id argument takes — its text on every edge serde writes, and a `uuid` in every
/// schema the other edges publish.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn an_id_is_one_graphql_type_on_a_reply_and_in_an_input() {
    let schema = hearing_schema();
    let introspected = schema
        .execute(
            r#"{
                read: __type(name: "Hearing") { fields { name type { kind name ofType { name } } } }
                write: __type(name: "CreateHearing") {
                    inputFields { name type { kind name ofType { name } } }
                }
            }"#,
        )
        .await
        .into_result()
        .expect("introspection answers")
        .data
        .into_json()
        .expect("the answer is JSON");
    let read = typed(&introspected["read"]["fields"]);
    assert_eq!(read["id"], ("ID", true), "{introspected}");
    assert_eq!(read["courtId"], ("ID", true), "{introspected}");
    assert_eq!(read["clerkId"], ("ID", false), "{introspected}");
    let write = typed(&introspected["write"]["inputFields"]);
    assert_eq!(write["courtId"], ("ID", true), "{introspected}");
    assert_eq!(write["clerkId"], ("ID", false), "{introspected}");

    let (answered, serialized) = as_a_reader_of_hearings(async {
        let answered = schema
            .execute("{ hearing { courtId clerkId } }")
            .await
            .into_result()
            .expect("the query answers")
            .data
            .into_json()
            .expect("the answer is JSON");
        let serialized = serde_json::to_value(hearing::Hearing::from(&hearing::row()))
            .expect("the row serializes");
        (answered, serialized)
    })
    .await;
    let court = json!(Uuid::from_u128(hearing::COURT));
    for (key, field) in [("court_id", "courtId"), ("clerk_id", "clerkId")] {
        assert_eq!(answered["hearing"][field], court, "{answered}");
        assert_eq!(serialized[key], court, "{serialized}");
    }

    let json_schema = nest_rs_seaorm::__private::schemars::schema_for!(hearing::Hearing);
    let json_schema = serde_json::to_value(&json_schema).expect("the schema serializes");
    let properties = &json_schema["properties"];
    assert_eq!(properties["court_id"]["type"], "string", "{json_schema}");
    assert_eq!(properties["court_id"]["format"], "uuid", "{json_schema}");
    assert_eq!(
        properties["clerk_id"]["type"],
        json!(["string", "null"]),
        "{json_schema}"
    );
    assert_eq!(properties["clerk_id"]["format"], "uuid", "{json_schema}");
}

/// What a wire row holds reads back as its column's own type — an id as the
/// `Uuid`, an instant as sea-orm's `DateTimeWithTimeZone` — so a field resolver
/// computing over its borrowed parent names no framework type.
#[cfg(feature = "graphql")]
#[test]
fn a_wire_row_reads_back_as_its_columns_types() {
    let row = hearing::row();
    let wire = hearing::Hearing::from(&row);
    let parent: &hearing::Hearing = &wire;
    let scheduled: &DateTimeWithTimeZone = parent.scheduled_at.as_ref();
    assert_eq!(*scheduled, row.scheduled_at);
    let court: &Uuid = parent.court_id.as_ref();
    assert_eq!(*court, row.court_id);
    assert_eq!(Uuid::from(parent.id), row.id);
    assert_eq!(parent.id.to_string(), row.id.to_string());
}
