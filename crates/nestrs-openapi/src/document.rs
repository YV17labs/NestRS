//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use nestrs_core::{Container, DiscoveryService};
use nestrs_http::{HttpControllerMeta, HttpRouteMeta, join_path};
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
    use super::*;

    #[test]
    fn joins_and_converts_paths() {
        assert_eq!(join_path("/users", "/:id"), "/users/:id");
        assert_eq!(openapi_path("/users/:id"), "/users/{id}");
        assert_eq!(join_path("/", "/"), "/");
    }

    #[test]
    fn derives_path_parameters() {
        let params = path_parameters("/users/:id");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "id");
        assert_eq!(params[0]["in"], "path");
    }
}
