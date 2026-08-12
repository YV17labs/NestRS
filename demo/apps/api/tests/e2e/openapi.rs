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

    let upload = &doc["paths"]["/audio/uploads/direct"]["post"];
    assert_eq!(
        upload["requestBody"]["content"]["multipart/form-data"]["schema"]["$ref"],
        "#/components/schemas/DirectUploadDto",
        "the direct upload documents the form it accepts: {upload}",
    );
    assert_eq!(
        doc["components"]["schemas"]["DirectUploadDto"]["properties"]["file"]["format"], "binary",
        "and its file part is typed as a file",
    );

    let download = &doc["paths"]["/audio/download"]["get"]["responses"]["200"];
    assert_eq!(
        download["content"]["audio/mpeg"]["schema"]["format"], "binary",
        "the streamed download declares what it streams: {download}",
    );

    let events = &doc["paths"]["/audio/events"]["get"];
    assert_eq!(
        events["responses"]["200"]["content"]["text/event-stream"]["schema"]["type"], "string",
        "the SSE route's media type comes off its return type: {events}",
    );
    let resume = events["parameters"]
        .as_array()
        .expect("the events op has parameters")
        .iter()
        .find(|p| p["in"] == "header")
        .expect("the resume header is documented");
    assert_eq!(resume["name"], "Last-Event-ID");
    assert_eq!(
        resume["required"], false,
        "a browser only sends it on reconnect: {resume}",
    );

    let results = &doc["paths"]["/audio/results"]["get"];
    assert_eq!(
        results["parameters"][0]["required"], true,
        "the transcode query names a file the caller must send: {results}",
    );
    assert_eq!(
        results["responses"]["400"]["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/ProblemDetails",
        "a required query property is a 400 the operation advertises: {results}",
    );

    let list = &doc["paths"]["/orgs"]["get"];
    assert_eq!(
        list["parameters"][0]["required"], false,
        "pagination is optional: {list}",
    );
    assert!(
        list["responses"].get("400").is_none(),
        "and an operation whose parameters are all optional advertises none: {list}",
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
