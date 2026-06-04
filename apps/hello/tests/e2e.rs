use hello::AppModule;
use nestrs_testing::TestApp;

#[tokio::test]
async fn hello_endpoint_greets() {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("AppModule boots and mounts its routes");

    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}
