//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use nest_rs_core::{Container, DiscoveryService};
use nest_rs_http::{GlobalGuardsActive, HttpConfig, HttpControllerMeta, HttpRouteMeta, join_path};
use poem::http::StatusCode;
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

    // The transport mounts everything under `HttpConfig.global_prefix`, but the
    // documented paths are relative to a controller's own prefix — so under a
    // global prefix every path in the document is wrong. Declare the prefix as
    // an OpenAPI `server` base URL: clients (and Swagger UI "Try it out")
    // prepend it to each path, keeping the paths themselves prefix-free (OAPI-O5).
    let mut document = json!({
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
    });

    if let Some(base) = global_prefix_base(container)
        && let Value::Object(obj) = &mut document
    {
        obj.insert("servers".into(), json!([{ "url": base }]));
    }

    document
}

/// The transport's `global_prefix`, normalized to a `server` base URL
/// (`/api`) — leading slash, no trailing slash — or `None` when unset.
fn global_prefix_base(container: &Container) -> Option<String> {
    let prefix = container.get::<HttpConfig>()?.global_prefix.clone()?;
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("/{trimmed}"))
    }
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
    // The effective success status (OAPI-O3): `#[http_code(201)]`, a `#[crud]`
    // delete's `204`, or a `#[redirect(_, 301)]` no longer masquerade as `200`.
    let status = route.success_status;
    let mut ok = Map::new();
    ok.insert("description".into(), json!(reason_phrase(status)));
    // A `204 No Content` and a `3xx` redirect carry no response body.
    let has_body = status != 204 && !(300..400).contains(&status);
    if has_body && let Some(schema_fn) = route.response {
        ok.insert(
            "content".into(),
            json!({ "application/json": { "schema": schema_fn(generator).to_value() } }),
        );
    }
    responses.insert(status.to_string(), Value::Object(ok));
    for (status, title) in error_statuses(route, full_path, global_guards) {
        let mut response = problem_response(title);
        // The throttler's `429` carries a `Retry-After` (seconds to window
        // reset) — document it so generated clients can honour the back-off.
        if status == "429"
            && let Value::Object(map) = &mut response
        {
            map.insert("headers".into(), retry_after_header());
        }
        responses.insert(status.into(), response);
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
/// path binds an id, `409` only on a `#[crud]` write, `400` only with a body.
fn error_statuses(
    route: &HttpRouteMeta,
    full_path: &str,
    global_guards: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if route.request_body.is_some() {
        // The framework's edge validation (`Valid`/`Piped`) rejects with a `400`
        // RFC-9457 problem+json (see `nest_rs_http::pipe::reject`), not `422` —
        // the generated document must state the status clients will actually see
        // (OAPI-O2).
        out.push(("400", "Bad Request"));
    }
    if route_is_guarded(route, global_guards) {
        out.push(("401", "Unauthorized"));
        out.push(("403", "Forbidden"));
    }
    // `404` where a path *segment* binds an id (`/users/:id`) — a lookup that
    // can miss. Match a leading-`:` segment, not any `:` in the string, so a
    // literal colon in a static segment doesn't spuriously advertise a `404`
    // (OAPI-O4). Driven off the path rather than `path_params` on purpose: a
    // `Bind<_, _>` route looks up its id and can 404 but carries no typed
    // `Path<…>` param, so `path_params` would be empty for exactly those routes.
    if full_path.split('/').any(|seg| seg.starts_with(':')) {
        out.push(("404", "Not Found"));
    }
    if route.may_conflict {
        out.push(("409", "Conflict"));
    }
    // `429` on a `ThrottlerGuard`-covered route — the guard answers with a
    // `Retry-After` header (added on the response below), so clients that read
    // the document know to back off (OAPI-O4).
    if route.throttled {
        out.push(("429", "Too Many Requests"));
    }
    out
}

/// Reason phrase for a success status, for the response `description`. Reuses
/// the `http` crate's canonical table (the same source `nest_rs_http::problem`
/// draws error phrases from) rather than a hand-kept copy that would drift.
fn reason_phrase(status: u16) -> &'static str {
    StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Success")
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

/// The `Retry-After` response header a throttled route's `429` carries: the
/// integer seconds a client should wait before retrying (RFC-9110 §10.2.3).
fn retry_after_header() -> Value {
    json!({
        "Retry-After": {
            "description": "Seconds to wait before retrying, until the rate-limit window resets.",
            "schema": { "type": "integer", "format": "int32", "minimum": 0 }
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
            throttled: false,
            success_status: 200,
            scoped_guarded: false,
            public: false,
        }
    }

    #[test]
    fn a_path_param_segment_advertises_404_but_a_literal_colon_does_not() {
        // OAPI-O4: `404` on a route that binds an id segment (`:id`) — Path OR
        // Bind — but not on a static segment that merely contains a colon.
        let bound = error_statuses(&route("get_user", "/users/:id"), "/users/:id", false);
        assert!(
            bound.iter().any(|(s, _)| *s == "404"),
            "an `:id` route advertises 404",
        );

        let literal = error_statuses(&route("weird", "/a:b/list"), "/a:b/list", false);
        assert!(
            !literal.iter().any(|(s, _)| *s == "404"),
            "a literal colon in a static segment must not advertise 404",
        );
    }

    #[test]
    fn a_throttled_route_advertises_429_with_a_retry_after_header() {
        // OAPI-O4: a `ThrottlerGuard`-covered route can answer `429`, and the
        // guard sends `Retry-After` — both must reach the document.
        let mut g = generator();
        let mut r = route("upload", "/audio/uploads");
        r.throttled = true;
        let op = operation_object(&r, "/audio/uploads", &mut g, false);
        let too_many = &op["responses"]["429"];
        assert_eq!(too_many["description"], "Too Many Requests");
        assert!(
            too_many["headers"]["Retry-After"]["schema"]["type"] == "integer",
            "429 must document the Retry-After header: {too_many}",
        );
    }

    #[test]
    fn an_unthrottled_route_does_not_advertise_429() {
        let statuses = error_statuses(&route("list", "/audio"), "/audio", false);
        assert!(
            !statuses.iter().any(|(s, _)| *s == "429"),
            "a route with no ThrottlerGuard must not advertise 429",
        );
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
    fn a_non_200_success_status_replaces_the_200_response() {
        // OAPI-O3: `#[http_code(201)]` advertises `201 Created`, not `200`, and
        // still carries the body schema.
        let mut g = generator();
        let mut r = route("create_user", "/users");
        r.success_status = 201;
        r.response = Some(schema_for_dummy);
        let op = operation_object(&r, "/users", &mut g, false);
        assert!(op["responses"].get("200").is_none(), "no bogus 200");
        assert_eq!(op["responses"]["201"]["description"], "Created");
        assert!(op["responses"]["201"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn a_204_or_redirect_success_carries_no_body() {
        // A `204 No Content` (a `#[crud]` delete) and a `3xx` redirect advertise
        // no response body even when a return schema exists (OAPI-O3).
        for (status, reason) in [(204, "No Content"), (307, "Temporary Redirect")] {
            let mut g = generator();
            let mut r = route("delete_user", "/users/:id");
            r.success_status = status;
            r.response = Some(schema_for_dummy);
            let op = operation_object(&r, "/users/:id", &mut g, false);
            let key = status.to_string();
            assert_eq!(op["responses"][&key]["description"], reason);
            assert!(
                op["responses"][&key].get("content").is_none(),
                "{status} must carry no response body",
            );
        }
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

    #[test]
    fn no_servers_field_without_a_global_prefix() {
        let container = Container::builder().build();
        let doc = build_document(&container, "X", "0", None);
        assert!(
            doc.get("servers").is_none(),
            "with no global prefix the paths are absolute — no `servers` needed",
        );
    }

    #[test]
    fn global_prefix_is_declared_as_a_normalized_server_base_url() {
        // OAPI-O5: under a global prefix the documented paths stay prefix-free
        // and the prefix rides in `servers`, so a client (and Swagger UI) is
        // prepended it correctly. The base is normalized (leading slash, no
        // trailing slash) regardless of how the operator wrote the prefix.
        for raw in ["api", "/api", "api/", "/api/"] {
            let container = Container::builder()
                .provide(HttpConfig::default().with_global_prefix(raw))
                .build();
            let doc = build_document(&container, "X", "0", None);
            assert_eq!(
                doc["servers"][0]["url"], "/api",
                "prefix {raw:?} must normalize to `/api`",
            );
        }
    }
}
