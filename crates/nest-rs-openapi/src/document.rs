//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use nest_rs_core::{Container, DiscoveryService};
use nest_rs_http::{GlobalGuardsActive, HttpControllerMeta, HttpRouteMeta, join_path};
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

    // A global guard pool (`use_guards_global`) covers every non-public route
    // even when no controller declares `#[use_guards]`, so the security scheme
    // and auth error responses must reflect it — mirroring how the transport
    // decides a route is implicitly guarded.
    let global_guards = container.get::<GlobalGuardsActive>().is_some();

    let mut paths: Map<String, Value> = Map::new();
    for controller in discovery.meta::<HttpControllerMeta>() {
        let prefix = controller.meta.effective_prefix();
        for route in &controller.meta.routes {
            let full = join_path(&prefix, route.path);
            let operation = operation_object(route, &full, &mut generator, global_guards);
            let item = paths
                .entry(openapi_path(&full))
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(methods) = item {
                methods.insert(route.verb.as_str().to_ascii_lowercase(), operation);
            }
        }
    }

    let mut schemas = generator.take_definitions(true);
    // The RFC 9457 error body every failure renders (see `nest_rs_http::problem`).
    // Hand-written rather than derived so the doc has no build-time dependency on
    // the concrete struct's schemars derive.
    schemas.insert("ProblemDetails".into(), problem_details_schema());

    let mut info = json!({ "title": title, "version": version });
    if let (Some(description), Value::Object(info)) = (description, &mut info) {
        info.insert("description".into(), json!(description));
    }

    json!({
        "openapi": "3.1.2",
        "info": info,
        "paths": Value::Object(paths),
        "components": {
            "schemas": Value::Object(schemas),
            // A guarded operation carries `security: [{ bearerAuth: [] }]`; a
            // `#[public]` one carries none — so a generated client can tell the
            // two apart (the gap this closes).
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                }
            },
        },
    })
}

/// The RFC 9457 `application/problem+json` schema referenced by every error
/// response. `errors` is the extension member field-level validation rides on.
fn problem_details_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "type": "string", "format": "uri" },
            "title": { "type": "string" },
            "status": { "type": "integer" },
            "detail": { "type": "string" },
            "instance": { "type": "string", "format": "uri" },
            "errors": { "type": "object", "additionalProperties": true },
        },
        "required": ["type", "title", "status"],
    })
}

/// Whether a route demands a bearer token in the document: it declares a
/// controller/method guard, or a global guard pool covers it — and it is not
/// `#[public]`.
fn route_is_guarded(route: &HttpRouteMeta, global_guards: bool) -> bool {
    (route.scoped_guarded || global_guards) && !route.public
}

fn operation_object(
    route: &HttpRouteMeta,
    full_path: &str,
    generator: &mut SchemaGenerator,
    global_guards: bool,
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

    let mut parameters = typed_path_parameters(full_path, route.path_params, generator);
    parameters.extend(expand_query_params(route.query_params, generator));
    if !parameters.is_empty() {
        op.insert("parameters".into(), Value::Array(parameters));
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

    // A guarded, non-public route demands a bearer token.
    if route_is_guarded(route, global_guards) {
        op.insert("security".into(), json!([{ "bearerAuth": [] }]));
    }

    let mut responses = Map::new();
    let mut ok = Map::new();
    ok.insert("description".into(), json!("OK"));
    if let Some(schema_fn) = route.response {
        ok.insert(
            "content".into(),
            json!({ "application/json": { "schema": schema_fn(generator).to_value() } }),
        );
    }
    responses.insert("200".into(), Value::Object(ok));
    for (status, title) in error_statuses(route, full_path, global_guards) {
        responses.insert(status.into(), problem_response(title));
    }
    op.insert("responses".into(), Value::Object(responses));

    Value::Object(op)
}

/// Path parameters typed from the handler's `Path<T>` extractor: the `i`-th
/// `:name` segment gets the schema of `path_params[i]`. Positional typing is
/// only applied when every segment has a matching `Path<T>` component; a handler
/// that binds some segments another way (`Bind<_, _>`, leaving fewer
/// `path_params` than segments) would misalign, so all segments fall back to the
/// `string`/`format: uuid` guess for an id-like name.
fn typed_path_parameters(
    path: &str,
    path_params: &[nest_rs_http::SchemaFn],
    generator: &mut SchemaGenerator,
) -> Vec<Value> {
    let names: Vec<&str> = path
        .split('/')
        .filter_map(|seg| seg.strip_prefix(':'))
        .collect();
    let positional = path_params.len() == names.len();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let schema = match positional.then(|| path_params.get(i)).flatten() {
                Some(schema_fn) => schema_fn(generator).to_value(),
                None if *name == "id" || name.ends_with("_id") => {
                    json!({ "type": "string", "format": "uuid" })
                }
                None => json!({ "type": "string" }),
            };
            json!({ "name": name, "in": "path", "required": true, "schema": schema })
        })
        .collect()
}

/// Expand each `Query<T>` payload into one `query` parameter per property of
/// `T`'s object schema — this is how the `#[crud]` list op's `Query<PageParams>`
/// surfaces `first` and `after`. A property absent from the schema's `required`
/// is an optional query parameter.
fn expand_query_params(
    query_params: &[nest_rs_http::SchemaFn],
    generator: &mut SchemaGenerator,
) -> Vec<Value> {
    let mut out = Vec::new();
    for schema_fn in query_params {
        // A named struct (`PageParams`) yields a `$ref`, not inline properties.
        // Build it against the *shared* generator so any nested struct/enum a
        // property references lands in the document's `components/schemas` — a
        // throwaway generator would drop those, leaving a dangling `$ref`.
        let schema = schema_fn(generator).to_value();
        let object = resolve_ref(&schema, generator.definitions());
        let required: Vec<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if let Some(props) = object.get("properties").and_then(Value::as_object) {
            for (name, prop_schema) in props {
                out.push(json!({
                    "name": name,
                    "in": "query",
                    "required": required.contains(&name.as_str()),
                    "schema": prop_schema,
                }));
            }
        }
    }
    out
}

/// Follow a top-level `{"$ref": "…/Name"}` to its definition; return the schema
/// unchanged when it is already inline.
fn resolve_ref(schema: &Value, defs: &Map<String, Value>) -> Value {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str)
        && let Some(name) = reference.rsplit('/').next()
        && let Some(def) = defs.get(name)
    {
        return def.clone();
    }
    schema.clone()
}

/// The error responses an operation can actually produce, as `(status, title)`.
/// Honest per route: auth codes only on a guarded route, `404` only where a
/// path binds an id, `409` only on a `#[crud]` write, `422` only with a body.
fn error_statuses(
    route: &HttpRouteMeta,
    full_path: &str,
    global_guards: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if route.request_body.is_some() {
        out.push(("422", "Unprocessable Content"));
    }
    if route_is_guarded(route, global_guards) {
        out.push(("401", "Unauthorized"));
        out.push(("403", "Forbidden"));
    }
    if full_path.contains(":") {
        out.push(("404", "Not Found"));
    }
    if route.may_conflict {
        out.push(("409", "Conflict"));
    }
    out
}

/// A single `application/problem+json` error response referencing the shared
/// `ProblemDetails` schema.
fn problem_response(title: &str) -> Value {
    json!({
        "description": title,
        "content": {
            "application/problem+json": {
                "schema": { "$ref": "#/components/schemas/ProblemDetails" }
            }
        }
    })
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
        let mut g = generator();
        // No `Path<T>` schema ⇒ an `id` segment falls back to `format: uuid`.
        let params = typed_path_parameters("/users/:id", &[], &mut g);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "id");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["type"], "string");
        assert_eq!(params[0]["schema"]["format"], "uuid");
    }

    #[test]
    fn path_parameters_is_empty_for_a_static_path() {
        let mut g = generator();
        assert!(typed_path_parameters("/health", &[], &mut g).is_empty());
        assert!(typed_path_parameters("/", &[], &mut g).is_empty());
    }

    #[test]
    fn path_parameters_emits_one_object_per_segment() {
        let mut g = generator();
        let params = typed_path_parameters("/orgs/:org_id/users/:id", &[], &mut g);
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
            path_params: &[],
            query_params: &[],
            may_conflict: false,
            scoped_guarded: false,
            public: false,
        }
    }

    #[test]
    fn operation_object_records_operation_id_and_tags() {
        let mut g = generator();
        let mut r = route("get_health", "/health");
        r.tags = &["health"];
        let op = operation_object(&r, "/health", &mut g, false);
        assert_eq!(op["operationId"], "get_health");
        assert_eq!(op["tags"][0], "health");
    }

    #[test]
    fn operation_object_skips_optional_metadata_when_absent() {
        let mut g = generator();
        let op = operation_object(&route("h", "/h"), "/h", &mut g, false);
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
        let op = operation_object(&r, "/h", &mut g, false);
        assert_eq!(op["summary"], "Quick");
        assert_eq!(op["description"], "Full prose");
    }

    #[test]
    fn operation_object_inlines_parameters_when_path_has_any() {
        let mut g = generator();
        let r = route("get_user", "/users/:id");
        let op = operation_object(&r, "/users/:id", &mut g, false);
        assert!(op["parameters"].is_array());
        assert_eq!(op["parameters"][0]["name"], "id");
    }

    #[test]
    fn operation_object_attaches_request_body_when_a_schema_fn_is_present() {
        let mut g = generator();
        let mut r = route("create_user", "/users");
        r.request_body = Some(schema_for_dummy);
        let op = operation_object(&r, "/users", &mut g, false);
        assert_eq!(op["requestBody"]["required"], true);
        assert!(op["requestBody"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn operation_object_always_emits_a_200_response_with_description() {
        let mut g = generator();
        let op = operation_object(&route("h", "/h"), "/h", &mut g, false);
        assert_eq!(op["responses"]["200"]["description"], "OK");
        // No `response` fn → no content block on 200.
        assert!(op["responses"]["200"].get("content").is_none());
    }

    #[test]
    fn operation_object_attaches_response_schema_when_present() {
        let mut g = generator();
        let mut r = route("get_user", "/users/:id");
        r.response = Some(schema_for_dummy);
        let op = operation_object(&r, "/users/:id", &mut g, false);
        assert!(op["responses"]["200"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn a_global_guard_pool_marks_an_otherwise_unguarded_route_as_secured() {
        // scoped_guarded=false, public=false: no controller/method guard, but a
        // `use_guards_global` pool covers it — the document must reflect that.
        let mut g = generator();
        let r = route("list", "/users");
        let op = operation_object(&r, "/users", &mut g, true);
        assert_eq!(op["security"][0]["bearerAuth"], json!([]));
        assert!(op["responses"].get("401").is_some());
        assert!(op["responses"].get("403").is_some());
    }

    #[test]
    fn a_public_route_stays_unsecured_even_under_a_global_guard_pool() {
        let mut g = generator();
        let mut r = route("health", "/health");
        r.public = true;
        let op = operation_object(&r, "/health", &mut g, true);
        assert!(op.get("security").is_none());
        assert!(op["responses"].get("401").is_none());
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
