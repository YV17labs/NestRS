//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use nest_rs_core::{Container, DiscoveryService};
use nest_rs_http::{HttpControllerMeta, HttpRouteMeta, join_path};
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

/// Build the OpenAPI document for everything mounted on the HTTP transport.
///
/// Called once at the transport's `configure` step (container fully assembled),
/// so it sees every controller. A single [`SchemaGenerator`] runs across all
/// routes so every `Json<T>` payload contributes to a shared
/// `components/schemas`.
pub fn build_document(
    container: &Container,
    title: &str,
    version: &str,
    description: Option<&str>,
) -> Value {
    let discovery = DiscoveryService::new(container);
    // OpenAPI 3.1 schema objects *are* JSON Schema 2020-12. The 3.0
    // `openapi3()` transforms (nullable/single-type rewrites) would corrupt the
    // output. Only `$ref`s are relocated to `#/components/schemas/...`.
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas".into();
    let mut generator = settings.into_generator();

    let mut paths: Map<String, Value> = Map::new();
    for controller in discovery.meta::<HttpControllerMeta>() {
        let prefix = controller.meta.effective_prefix();
        for route in &controller.meta.routes {
            let full = join_path(&prefix, route.path);
            let operation = operation_object(route, &path_parameters(&full), &mut generator);
            let item = paths
                .entry(openapi_path(&full))
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(methods) = item {
                methods.insert(route.verb.as_str().to_ascii_lowercase(), operation);
            }
        }
    }

    let schemas = generator.take_definitions(true);

    let mut info = json!({ "title": title, "version": version });
    if let (Some(description), Value::Object(info)) = (description, &mut info) {
        info.insert("description".into(), json!(description));
    }

    json!({
        "openapi": "3.1.2",
        "info": info,
        "paths": Value::Object(paths),
        "components": { "schemas": Value::Object(schemas) },
    })
}

fn operation_object(
    route: &HttpRouteMeta,
    parameters: &[Value],
    generator: &mut SchemaGenerator,
) -> Value {
    let mut op = Map::new();
    op.insert("operationId".into(), json!(route.handler));
    op.insert("tags".into(), json!(route.tags));
    if let Some(summary) = route.summary {
        op.insert("summary".into(), json!(summary));
    }
    if let Some(description) = route.description {
        op.insert("description".into(), json!(description));
    }
    if !parameters.is_empty() {
        op.insert("parameters".into(), Value::Array(parameters.to_vec()));
    }
    if let Some(schema_fn) = route.request_body {
        op.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": { "application/json": { "schema": schema_fn(generator).to_value() } },
            }),
        );
    }

    let mut ok = Map::new();
    // OpenAPI requires a response description; per-response text isn't modeled yet.
    ok.insert("description".into(), json!("OK"));
    if let Some(schema_fn) = route.response {
        ok.insert(
            "content".into(),
            json!({ "application/json": { "schema": schema_fn(generator).to_value() } }),
        );
    }
    op.insert("responses".into(), json!({ "200": Value::Object(ok) }));

    Value::Object(op)
}

fn path_parameters(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix(':'))
        .map(|name| {
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            })
        })
        .collect()
}

/// poem path syntax (`/users/:id`) → OpenAPI syntax (`/users/{id}`).
fn openapi_path(path: &str) -> String {
    path.split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => seg.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use nest_rs_http::HttpVerb;
    use schemars::JsonSchema;
    use schemars::generate::SchemaSettings;
    use serde::Serialize;

    use super::*;

    #[test]
    fn joins_and_converts_paths() {
        assert_eq!(join_path("/users", "/:id"), "/users/:id");
        assert_eq!(openapi_path("/users/:id"), "/users/{id}");
        assert_eq!(join_path("/", "/"), "/");
    }

    #[test]
    fn openapi_path_handles_root_and_no_params() {
        assert_eq!(openapi_path("/"), "/");
        assert_eq!(openapi_path("/users"), "/users");
        assert_eq!(openapi_path(""), "");
    }

    #[test]
    fn openapi_path_handles_multiple_params() {
        assert_eq!(
            openapi_path("/orgs/:org_id/users/:id"),
            "/orgs/{org_id}/users/{id}",
        );
    }

    #[test]
    fn derives_path_parameters() {
        let params = path_parameters("/users/:id");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "id");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["type"], "string");
    }

    #[test]
    fn path_parameters_is_empty_for_a_static_path() {
        assert!(path_parameters("/health").is_empty());
        assert!(path_parameters("/").is_empty());
    }

    #[test]
    fn path_parameters_emits_one_object_per_segment() {
        let params = path_parameters("/orgs/:org_id/users/:id");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["name"], "org_id");
        assert_eq!(params[1]["name"], "id");
    }

    // Building an `HttpRouteMeta` from outside `nest-rs-http` is awkward —
    // build a minimal one via `Default` if possible, else thread real values.
    fn generator() -> SchemaGenerator {
        let mut settings = SchemaSettings::draft2020_12();
        settings.definitions_path = "/components/schemas".into();
        settings.into_generator()
    }

    #[derive(Serialize, JsonSchema)]
    struct DummyBody {
        name: String,
    }

    fn schema_for_dummy(generator: &mut SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<DummyBody>()
    }

    fn route(handler: &'static str, path: &'static str) -> HttpRouteMeta {
        HttpRouteMeta {
            verb: HttpVerb::Get,
            path,
            handler,
            tags: &[],
            summary: None,
            description: None,
            request_body: None,
            response: None,
            scoped_guarded: false,
            public: false,
        }
    }

    #[test]
    fn operation_object_records_operation_id_and_tags() {
        let mut g = generator();
        let mut r = route("get_health", "/health");
        r.tags = &["health"];
        let op = operation_object(&r, &[], &mut g);
        assert_eq!(op["operationId"], "get_health");
        assert_eq!(op["tags"][0], "health");
    }

    #[test]
    fn operation_object_skips_optional_metadata_when_absent() {
        let mut g = generator();
        let op = operation_object(&route("h", "/h"), &[], &mut g);
        let obj = op.as_object().unwrap();
        assert!(!obj.contains_key("summary"));
        assert!(!obj.contains_key("description"));
        assert!(!obj.contains_key("parameters"));
        assert!(!obj.contains_key("requestBody"));
    }

    #[test]
    fn operation_object_includes_summary_and_description_when_set() {
        let mut g = generator();
        let mut r = route("h", "/h");
        r.summary = Some("Quick");
        r.description = Some("Full prose");
        let op = operation_object(&r, &[], &mut g);
        assert_eq!(op["summary"], "Quick");
        assert_eq!(op["description"], "Full prose");
    }

    #[test]
    fn operation_object_inlines_parameters_when_path_has_any() {
        let mut g = generator();
        let r = route("get_user", "/users/:id");
        let params = path_parameters("/users/:id");
        let op = operation_object(&r, &params, &mut g);
        assert!(op["parameters"].is_array());
        assert_eq!(op["parameters"][0]["name"], "id");
    }

    #[test]
    fn operation_object_attaches_request_body_when_a_schema_fn_is_present() {
        let mut g = generator();
        let mut r = route("create_user", "/users");
        r.request_body = Some(schema_for_dummy);
        let op = operation_object(&r, &[], &mut g);
        assert_eq!(op["requestBody"]["required"], true);
        assert!(op["requestBody"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn operation_object_always_emits_a_200_response_with_description() {
        let mut g = generator();
        let op = operation_object(&route("h", "/h"), &[], &mut g);
        assert_eq!(op["responses"]["200"]["description"], "OK");
        // No `response` fn → no content block on 200.
        assert!(op["responses"]["200"].get("content").is_none());
    }

    #[test]
    fn operation_object_attaches_response_schema_when_present() {
        let mut g = generator();
        let mut r = route("get_user", "/users/:id");
        r.response = Some(schema_for_dummy);
        let op = operation_object(&r, &[], &mut g);
        assert!(op["responses"]["200"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn build_document_emits_openapi_3_1_with_info_and_no_paths_for_empty_discovery() {
        let container = Container::builder().build();
        let doc = build_document(&container, "Test API", "1.2.3", None);
        assert_eq!(doc["openapi"], "3.1.2");
        assert_eq!(doc["info"]["title"], "Test API");
        assert_eq!(doc["info"]["version"], "1.2.3");
        assert!(doc["info"].get("description").is_none());
        assert!(doc["paths"].is_object());
        assert!(doc["components"]["schemas"].is_object());
    }

    #[test]
    fn build_document_carries_description_when_supplied() {
        let container = Container::builder().build();
        let doc = build_document(&container, "X", "0", Some("a description"));
        assert_eq!(doc["info"]["description"], "a description");
    }
}
