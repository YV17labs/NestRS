use hello::HelloModule;
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[tokio::test]
async fn hello_endpoint_greets() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots and mounts its routes");

    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}

#[tokio::test]
async fn http_code_overrides_status_and_response_header_is_appended() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots");

    let resp = app.http().post("/echo").send().await;
    resp.assert_status(StatusCode::CREATED);
    resp.assert_header("x-powered-by", "nestrs");
    resp.assert_text("Hello World").await;
}

#[tokio::test]
async fn redirect_emits_status_and_location_header() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots");

    let resp = app.http().get("/docs").send().await;
    resp.assert_status(StatusCode::MOVED_PERMANENTLY);
    resp.assert_header("location", "https://docs.nestrs.dev");
}

/// Bug 1 + Bug 5: a Result-returning handler decorated with `#[http_code]`
/// must keep the Err's `ResponseError` status (403 here) instead of being
/// silently rewritten to 201. This also exercises Bug 5: the wrapper now
/// compiles a `Result<String, ForbiddenError>` even though `Result<_, _>`
/// is not itself `IntoResponse`.
#[tokio::test]
async fn http_code_does_not_override_the_status_of_err_responses() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots");

    let resp = app.http().post("/forbidden").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

/// Bug 11: `#[response_header(name, value)]` on a single-valued header must
/// override whatever the handler already set, not append a second entry.
/// Pre-fix, the response carried two `content-type` headers.
#[tokio::test]
async fn response_header_overrides_a_handler_set_header() {
    let app = TestApp::for_module::<HelloModule>()
        .await
        .expect("HelloModule boots");

    let resp = app.http().get("/xml-as-json").send().await;
    resp.assert_status_is_ok();
    resp.assert_header_all("content-type", ["application/json"]);
}
