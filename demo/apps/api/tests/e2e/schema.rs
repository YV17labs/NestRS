use nest_rs::config::{Environment, Surface};
use nest_rs::graphql::GraphqlConfig;
use nest_rs::testing::TestApp;
use uuid::Uuid;

#[tokio::test]
async fn the_committed_schema_is_the_one_the_app_serves() {
    let emitted =
        std::env::temp_dir().join(format!("nestrs-api-schema-{}.graphql", Uuid::now_v7()));
    let _app = crate::boot_with(TestApp::builder().provide(GraphqlConfig {
        emit: Surface::Open,
        emit_path: emitted.clone(),
        ..Default::default()
    }))
    .await;
    let served = std::fs::read_to_string(&emitted).expect("the boot wrote the schema it serves");
    std::fs::remove_file(&emitted).expect("the emitted schema is deleted");

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.graphql");
    let committed = std::fs::read_to_string(path).expect("apps/api/schema.graphql is committed");

    assert!(
        served == committed,
        "apps/api/schema.graphql is not what the api serves: run the api once under \
         {}=development, which rewrites it, and commit the diff",
        Environment::var_name(),
    );
}
