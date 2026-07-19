//! The generated OpenAPI document.

use serde_json::json;

use super::harness::*;

#[tokio::test]
async fn openapi_document_describes_the_routes() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/api-json").send().await;
    resp.assert_status_is_ok();
    let bytes = resp.0.into_body().into_bytes().await.expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("api-json is JSON");

    let paths = doc["paths"].as_object().expect("paths object");
    assert!(paths.contains_key("/orgs"), "paths include /orgs");
    assert!(paths.contains_key("/users"), "paths include /users");

    assert_eq!(
        doc["components"]["securitySchemes"]["bearerAuth"]["scheme"], "bearer",
        "bearerAuth security scheme is declared",
    );
    assert_eq!(
        doc["paths"]["/orgs"]["get"]["security"][0]["bearerAuth"],
        json!([]),
        "the guarded list route requires bearerAuth",
    );

    let params = doc["paths"]["/orgs"]["get"]["parameters"]
        .as_array()
        .expect("list op has parameters");
    let query_names: Vec<&str> = params
        .iter()
        .filter(|p| p["in"] == "query")
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        query_names.contains(&"first") && query_names.contains(&"after"),
        "pagination cursor is documented as query params: {query_names:?}",
    );

    assert_eq!(
        doc["paths"]["/orgs/{id}"]["get"]["parameters"][0]["schema"]["format"], "uuid",
        "the :id path param is typed uuid",
    );

    let create = &doc["paths"]["/orgs"]["post"]["responses"];
    for status in ["401", "403", "409", "422"] {
        assert_eq!(
            create[status]["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDetails",
            "create advertises a problem+json {status} response",
        );
    }
}
