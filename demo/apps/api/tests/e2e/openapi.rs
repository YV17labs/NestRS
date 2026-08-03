use serde_json::json;

use crate::*;

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

    // OAPI-O5: the generated CRUD surface types its **responses**, not only its
    // inputs. It used to publish none — an ability shaper was read as "the
    // field set depends on the caller, so say nothing" — which typed every
    // `#[crud]` response as `any` in a generated client, on exactly the surface
    // `#[expose]` exists to serve. The document now carries the shape and says
    // the fields are ability-dependent.
    let list_ok = &doc["paths"]["/orgs"]["get"]["responses"]["200"];
    assert_eq!(
        list_ok["content"]["application/json"]["schema"]["items"]["$ref"],
        "#/components/schemas/Org",
        "the list response is an array of the exposed entity: {list_ok}",
    );
    assert_eq!(
        doc["paths"]["/orgs/{id}"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/Org",
        "the by-id response is the exposed entity",
    );
    assert!(
        doc["components"]["schemas"]["Org"].is_object(),
        "the output entity is a component, not just the create/update inputs",
    );
    assert!(
        list_ok["description"]
            .as_str()
            .expect("a description")
            .contains("ability"),
        "a masked response says its field set is ability-dependent: {list_ok}",
    );

    // A route that mints a resource answers `201 Created`, and the document
    // says so rather than advertising a `200` the wire never sends.
    let created = &doc["paths"]["/orgs"]["post"]["responses"]["201"];
    assert_eq!(
        created["content"]["application/json"]["schema"]["$ref"], "#/components/schemas/Org",
        "create advertises 201 with the created entity: {created}",
    );
    assert!(
        doc["paths"]["/orgs"]["post"]["responses"]
            .get("200")
            .is_none(),
        "no bogus 200 next to the 201",
    );
    // …and the `Location` it ships is declared, for the reason the `429` below
    // declares its `Retry-After`: a generated client reads the document.
    assert_eq!(
        created["headers"]["Location"]["schema"]["format"], "uri-reference",
        "the 201 documents the Location header it sends: {created}",
    );

    let create = &doc["paths"]["/orgs"]["post"]["responses"];
    for status in ["400", "401", "403", "409"] {
        assert_eq!(
            create[status]["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDetails",
            "create advertises a problem+json {status} response",
        );
    }

    let delete = &doc["paths"]["/orgs/{id}"]["delete"]["responses"];
    assert!(
        delete.get("204").is_some() && delete.get("200").is_none(),
        "delete advertises 204, not 200: {delete}",
    );
    assert!(
        delete["204"].get("content").is_none(),
        "the 204 response carries no body",
    );

    let throttled = &doc["paths"]["/audio/uploads"]["post"]["responses"]["429"];
    assert_eq!(
        throttled["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/ProblemDetails",
        "a throttled route advertises a problem+json 429: {throttled}",
    );
    assert_eq!(
        throttled["headers"]["Retry-After"]["schema"]["type"], "integer",
        "the 429 documents an integer Retry-After header: {throttled}",
    );
}
