use hello::HelloModule;
use nestrs_testing::TestApp;

#[tokio::test]
async fn hello_endpoint_greets() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots and mounts its routes");

    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}
