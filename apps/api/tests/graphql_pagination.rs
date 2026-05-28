//! `#[expose(paginate)]` end-to-end: a locally-exposed entity, its generated
//! `*Page` envelope, the shared `PageArgs` input, and a `#[query]` returning the
//! page all compose into the live GraphQL schema and resolve with the right
//! pagination math — driven through the in-process harness, no database. The
//! resolver/entity are defined here (not pulled from the app) so their
//! self-registration is retained in this test binary.

use nestrs_core::module;
use nestrs_graphql::async_graphql::Result as GqlResult;
use nestrs_graphql::{resolver, GraphqlModule};
use nestrs_http::HttpTransport;
use nestrs_resource::{expose, PageArgs};
use nestrs_testing::TestApp;
use serde::Deserialize;
use serde_json::json;

// A minimal "entity": `#[expose(paginate)]` emits `Widget` (GraphQL object) and
// `WidgetPage` (the envelope). No SeaORM needed — the macro only reads fields.
#[expose(name = "Widget", paginate)]
struct WidgetModel {
    id: i32,
    name: String,
}

#[resolver]
struct WidgetResolver;

#[resolver]
impl WidgetResolver {
    // A page over a fixed five-row "table", so the math is deterministic and
    // needs no database.
    #[query]
    async fn widgets_page(&self, args: PageArgs) -> GqlResult<WidgetPage> {
        let table = ["a", "b", "c", "d", "e"];
        let total = table.len() as u64;
        let items: Vec<Widget> = table
            .iter()
            .enumerate()
            .skip(args.offset() as usize)
            .take(args.limit() as usize)
            .map(|(i, n)| {
                Widget::from(&WidgetModel {
                    id: i as i32,
                    name: (*n).to_string(),
                })
            })
            .collect();
        Ok(WidgetPage::new(items, total, &args))
    }
}

#[module(imports = [GraphqlModule])]
struct SchemaModule;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageView {
    items: Vec<ItemView>,
    total: u64,
    page: u64,
    per_page: u64,
    total_pages: u64,
    has_next_page: bool,
    has_previous_page: bool,
}

#[derive(Deserialize)]
struct ItemView {
    name: String,
}

async fn run(query: &str) -> PageView {
    let app = TestApp::builder()
        .module::<SchemaModule>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    json.value()
        .object()
        .get("data")
        .object()
        .get("widgetsPage")
        .deserialize()
}

#[tokio::test]
async fn first_page_reports_more_to_come() {
    let page = run("{ widgetsPage(args: { page: 1, perPage: 2 }) { items { name } total page perPage totalPages hasNextPage hasPreviousPage } }").await;
    assert_eq!(page.total, 5);
    assert_eq!(page.page, 1);
    assert_eq!(page.per_page, 2);
    assert_eq!(page.total_pages, 3); // ceil(5 / 2)
    assert!(page.has_next_page);
    assert!(!page.has_previous_page);
    let names: Vec<&str> = page.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test]
async fn last_page_is_partial_and_has_no_next() {
    let page = run("{ widgetsPage(args: { page: 3, perPage: 2 }) { items { name } total page perPage totalPages hasNextPage hasPreviousPage } }").await;
    assert_eq!(page.total, 5);
    assert_eq!(page.page, 3);
    assert_eq!(page.total_pages, 3);
    assert!(!page.has_next_page);
    assert!(page.has_previous_page);
    let names: Vec<&str> = page.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["e"]); // offset 4, only one row left
}
