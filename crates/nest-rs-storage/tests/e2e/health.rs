//! The indicator `StorageHealthModule` attaches (`src/health/`), against the
//! live store: a bucket removed under a running app drops readiness to `503`
//! naming `storage`, and liveness waits on nothing. The bucket is the test's
//! own, created and removed through the store's API: the suite's `nestrs`
//! bucket holds every other test's objects.

use std::time::Duration;

use http::Method;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::StatusCode;
use nest_rs_storage::{StorageConfig, StorageHealthModule, StorageModule};
use nest_rs_testing::{TestApp, wait_for};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{Certificate, ClientOptions};

use crate::{config, unique};

#[module(imports = [HealthModule, StorageModule, StorageHealthModule])]
struct ProbedStore;

/// Send `method` to the root of `bucket` — `PUT` creates it, `DELETE` removes
/// it — signed with the suite's credentials, and read the store's status.
async fn at_the_root_of(bucket: &str, method: Method) -> u16 {
    let suite = config();
    let authorities = Certificate::from_pem_bundle(suite.tls.authorities_pem())
        .expect("the suite's authorities parse");
    let options = authorities.into_iter().fold(
        ClientOptions::new().with_no_system_certificates(true),
        ClientOptions::with_root_certificate,
    );
    let mut builder = AmazonS3Builder::new()
        .with_client_options(options)
        .with_region(&suite.region)
        .with_access_key_id(&suite.access_key)
        .with_secret_access_key(&suite.secret_key)
        .with_bucket_name(bucket)
        .with_virtual_hosted_style_request(!suite.force_path_style);
    if let Some(endpoint) = &suite.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    let signer = builder.build().expect("a client of the suite's store");
    let url = signer
        .signed_url(method.clone(), &Path::from(""), Duration::from_secs(60))
        .await
        .expect("a signed URL for the bucket's root");
    reqwest::Client::new()
        .request(method, url)
        .send()
        .await
        .expect("the store answers")
        .status()
        .as_u16()
}

#[tokio::test]
async fn a_bucket_removed_under_a_running_app_drops_readiness_naming_the_store() {
    let bucket = unique("health");
    let created = at_the_root_of(&bucket, Method::PUT).await;
    assert!(
        (200..300).contains(&created),
        "the bucket is created: {created}"
    );
    let app = TestApp::builder()
        .provide(StorageConfig {
            url: format!("s3://{bucket}"),
            ..config()
        })
        .module::<ProbedStore>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("boots over its own bucket: {refused:#}"));
    let ready = app.http().get("/health/ready").send().await;
    ready.assert_status_is_ok();
    ready
        .json()
        .await
        .value()
        .object()
        .get("info")
        .object()
        .get("storage")
        .object()
        .get("status")
        .assert_string("up");

    let removed = at_the_root_of(&bucket, Method::DELETE).await;
    assert!(
        (200..300).contains(&removed),
        "the bucket is removed: {removed}"
    );
    wait_for(Duration::from_secs(5), || {
        let app = &app;
        async move {
            app.http().get("/health/ready").send().await.0.status()
                == StatusCode::SERVICE_UNAVAILABLE
        }
    })
    .await;

    let ready = app.http().get("/health/ready").send().await;
    ready.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    ready
        .json()
        .await
        .value()
        .object()
        .get("error")
        .object()
        .get("storage")
        .object()
        .get("status")
        .assert_string("down");
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}
