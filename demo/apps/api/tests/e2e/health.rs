use super::harness::*;

#[tokio::test]
async fn health_live_probe_is_ok() {
    let (_db, app) = boot().await;
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn health_ready_probe_reports_db_indicator_up() {
    let (_db, app) = boot().await;
    app.init()
        .await
        .expect("lifecycle init wires the indicator registry");
    let resp = app.http().get("/health/ready").send().await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    let body = body.value().object();
    assert_eq!(body.get("status").string(), "up");
    assert!(
        body.get("info").object().get_opt("db").is_some(),
        "ready probe info bucket carries the `db` indicator",
    );
    assert!(
        body.get("error").object().is_empty(),
        "ready probe error bucket is empty against a live database",
    );
}
