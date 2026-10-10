//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::{Container, Discovery};
use nest_rs_http::{
    ApiVersioning, GlobalGuardsActive, HttpConfig, HttpControllerMeta, HttpRouteMeta,
    MEDIA_TYPE_PARAM, RouteTemplate, declared_versions, join_path,
};
use poem::http::{StatusCode, header};
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::config::OpenApiConfig;

/// The `operationId` collisions already reported during one boot, shared by
/// every document built from one route table.
#[derive(Default)]
pub(crate) struct Reported {
    ids: std::collections::HashSet<String>,
}

/// Build the OpenAPI document for everything mounted on the HTTP transport.
///
/// `claims` is the version described under a non-URI strategy: `None` claims the
/// default version, or every declared one; ignored under the URI strategy.
pub(crate) fn build_document(
    container: &Container,
    config: &OpenApiConfig,
    claims: Option<&str>,
    reported: &mut Reported,
) -> Value {
    let discovery = Discovery::new(container);
    // OpenAPI 3.1 schemas are JSON Schema 2020-12: `openapi3()`'s rewrites would corrupt them.
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas".into();
    let mut generator = settings.into_generator();

    let global_guards = container.get::<GlobalGuardsActive>().is_some();
    let selection = VersionSelection::resolve(container, claims);

    let controllers = discovery.meta::<HttpControllerMeta>();
    let mut entries: Vec<(&Arc<HttpControllerMeta>, Option<&'static str>)> = controllers
        .iter()
        .flat_map(|d| d.meta.mounted_versions().map(move |v| (&d.meta, v)))
        .collect();
    if selection.is_some() {
        // The last write wins a contested path, so the highest version is described.
        // Not under the URI strategy: reordering would rewrite a committed document.
        entries.sort_by(|(a_meta, a), (b_meta, b)| {
            compare_versions(*a, *b).then_with(|| a_meta.path.cmp(b_meta.path))
        });
    }

    let mut described: HashMap<(String, &'static str), Option<&'static str>> = HashMap::new();
    let mut operation_ids: HashMap<String, (String, &'static str)> = HashMap::new();
    let mut paths: Map<String, Value> = Map::new();
    for (meta, version) in &entries {
        let version = *version;
        if let Some(selection) = &selection
            && !selection.describes(version)
        {
            continue;
        }
        // Under a non-URI strategy the mounted `/v1/posts` answers `404`.
        let prefix = match &selection {
            Some(_) => meta.path.to_owned(),
            None => meta.effective_prefix(version),
        };
        for route in &meta.routes {
            if !HttpControllerMeta::serves(route, version) {
                continue;
            }
            let full = join_path(&prefix, route.path);
            let full = match RouteTemplate::parse(&full) {
                Ok(template) => template,
                Err(refused) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        controller = meta.controller,
                        handler = route.handler,
                        error = %nest_rs_core::error_message(&refused),
                        "route omitted from the document: its path is not a route template",
                    );
                    continue;
                }
            };
            let Some(key) = openapi_path(&full).map(str::to_owned) else {
                tracing::warn!(
                    target: crate::TARGET,
                    controller = meta.controller,
                    handler = route.handler,
                    path = %full,
                    "route omitted from the document: an OpenAPI path template names one \
                     segment's value, so a catch-all or a literal brace cannot be described",
                );
                continue;
            };
            let version_parameter = match (&selection, version) {
                (Some(selection), Some(version)) => Some(selection.parameter(version, route)),
                _ => None,
            };
            let id = operation_id(meta.token, route.handler, version);
            let operation = operation_object(
                route,
                &full,
                &id,
                &mut generator,
                global_guards,
                version_parameter,
            );
            // OpenAPI 3.1 §4.8.10.1: an id is unique across the document. One id on one
            // address is a duplicate mount, the transport's to name.
            let address = (key.clone(), route.verb.as_str());
            if let Some(previous) = operation_ids.insert(id.clone(), address.clone())
                && previous != address
                && reported.ids.insert(id.clone())
            {
                tracing::warn!(
                    target: crate::TARGET,
                    operation_id = id.as_str(),
                    path = key.as_str(),
                    method = route.verb.as_str(),
                    conflicting_path = previous.0.as_str(),
                    conflicting_method = previous.1,
                    hint = DUPLICATE_ID_REMEDY,
                    "two operations share one operationId",
                );
            }
            // OpenAPI keys one operation per (path, method): name the version dropped.
            if selection.is_some()
                && let Some(previous) =
                    described.insert((key.clone(), route.verb.as_str()), version)
                && previous != version
            {
                tracing::warn!(
                    target: crate::TARGET,
                    path = key.as_str(),
                    method = route.verb.as_str(),
                    described = version_label(version),
                    omitted = version_label(previous),
                    hint = contested_path_remedy().as_str(),
                    "two API versions serve one documented path",
                );
            }
            let item = paths
                .entry(key)
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(methods) = item {
                methods.insert(route.verb.as_str().to_ascii_lowercase(), operation);
            }
        }
    }

    let mut schemas = generator.take_definitions(true);
    schemas.insert("ProblemDetails".into(), problem_details_schema());

    let info = info_object(config);

    // Paths stay prefix-free; `global_prefix` becomes the `server` base URL clients prepend.
    let mut document = json!({
        "openapi": "3.1.2",
        "info": info,
        "paths": Value::Object(paths),
        "components": {
            "schemas": Value::Object(schemas),
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                }
            },
        },
    });

    // Stated even at the origin root, OpenAPI's default: a generator reads its
    // base URL off `servers` and has none to read otherwise.
    if let Value::Object(obj) = &mut document {
        let base = global_prefix_base(container).unwrap_or_else(|| "/".to_owned());
        obj.insert("servers".into(), json!([{ "url": base }]));
    }

    retain_reachable_schemas(&mut document);
    document
}

/// Drop every component schema no operation reaches. A `Query<T>` or
/// `Header<T>` payload is expanded into parameters, so its own definition is
/// referenced by nothing, and a generator would emit a type no call uses.
fn retain_reachable_schemas(document: &mut Value) {
    let mut pending = Vec::new();
    collect_schema_refs(&document["paths"], &mut pending);
    let Some(schemas) = document
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let mut reachable = std::collections::HashSet::new();
    while let Some(name) = pending.pop() {
        if reachable.insert(name.clone())
            && let Some(schema) = schemas.get(&name)
        {
            collect_schema_refs(schema, &mut pending);
        }
    }
    schemas.retain(|name, _| reachable.contains(name));
}

/// The component names every `$ref` under `value` points at.
fn collect_schema_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/components/schemas/"))
            {
                // RFC 6901 §4: `~1` before `~0`.
                out.push(name.replace("~1", "/").replace("~0", "~"));
            }
            for child in map.values() {
                collect_schema_refs(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_schema_refs(child, out);
            }
        }
        _ => {}
    }
}

/// OpenAPI's Info Object, every field the config sets.
fn info_object(config: &OpenApiConfig) -> Value {
    let mut info = Map::new();
    info.insert("title".into(), json!(config.title));
    if let Some(summary) = &config.summary {
        info.insert("summary".into(), json!(summary));
    }
    if let Some(description) = &config.description {
        info.insert("description".into(), json!(description));
    }
    if let Some(terms) = &config.terms_of_service {
        info.insert("termsOfService".into(), json!(terms));
    }
    if let Some(contact) = &config.contact {
        let mut object = Map::new();
        for (key, value) in [
            ("name", &contact.name),
            ("url", &contact.url),
            ("email", &contact.email),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), json!(value));
            }
        }
        info.insert("contact".into(), Value::Object(object));
    }
    if let Some(license) = &config.license {
        let mut object = Map::new();
        object.insert("name".into(), json!(license.name));
        if let Some(identifier) = &license.identifier {
            object.insert("identifier".into(), json!(identifier));
        }
        if let Some(url) = &license.url {
            object.insert("url".into(), json!(url));
        }
        info.insert("license".into(), Value::Object(object));
    }
    info.insert("version".into(), json!(config.version));
    Value::Object(info)
}

fn global_prefix_base(container: &Container) -> Option<String> {
    let prefix = container.get::<HttpConfig>()?.global_prefix.clone()?;
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("/{trimmed}"))
    }
}

pub(crate) fn versioned_documents(container: &Container) -> Vec<String> {
    match selects_per_request(container) {
        true => declared_versions(container),
        false => Vec::new(),
    }
}

pub(crate) fn selects_per_request(container: &Container) -> bool {
    container
        .get::<HttpConfig>()
        .is_some_and(|config| config.versioning != ApiVersioning::Uri)
}

/// How a document tells a caller to ask for an API version when the version is
/// not in the path: as a header parameter on the unversioned address.
struct VersionSelection {
    strategy: ApiVersioning,
    header: String,
    required: bool,
    /// The one version this document describes, or `None` for every one.
    claims: Option<String>,
}

impl VersionSelection {
    fn resolve(container: &Container, claims: Option<&str>) -> Option<Self> {
        let config = container.get::<HttpConfig>()?;
        if config.versioning == ApiVersioning::Uri {
            return None;
        }
        Some(Self {
            strategy: config.versioning,
            header: match config.versioning {
                ApiVersioning::MediaType => header::ACCEPT.as_str().to_owned(),
                _ => config.version_header.clone(),
            },
            required: config.default_version.is_none(),
            claims: claims
                .map(str::to_owned)
                .or_else(|| config.default_version.clone()),
        })
    }

    fn describes(&self, version: Option<&str>) -> bool {
        match (version, &self.claims) {
            (Some(version), Some(claims)) => version == claims,
            _ => true,
        }
    }

    fn parameter(&self, version: &str, route: &HttpRouteMeta) -> Value {
        let (description, accepted) = match self.strategy {
            ApiVersioning::MediaType => (
                format!(
                    "Selects the API version, as the `{MEDIA_TYPE_PARAM}` parameter of the media \
                     range this operation is requested under.",
                ),
                format!(
                    "{}; {MEDIA_TYPE_PARAM}={version}",
                    route.response_content_type.unwrap_or(JSON_MEDIA_TYPE),
                ),
            ),
            _ => (
                "Selects the API version this operation is served under.".to_owned(),
                version.to_owned(),
            ),
        };
        json!({
            "name": self.header,
            "in": "header",
            "required": self.required,
            "description": description,
            "schema": { "type": "string", "enum": [accepted] },
        })
    }
}

fn version_label(version: Option<&str>) -> &str {
    version.unwrap_or("unversioned")
}

/// The `operationId` an operation is published under: `posts_list`, `posts_list_v1`.
fn operation_id(token: &str, handler: &str, version: Option<&str>) -> String {
    let qualified = format!("{token}_{handler}");
    let id = match version {
        Some(version) => format!("{qualified}_v{version}"),
        None => qualified,
    };
    identifier_token(&id)
}

/// An `operationId` as a generated client can carry it: every character an
/// identifier cannot hold becomes `_`. Run over the whole id: every part, raw
/// idents (`r#type`) included, can carry one.
fn identifier_token(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

const DUPLICATE_ID_REMEDY: &str = "rename one of the two controllers, or one of the two \
     handlers: an operationId is <controller>_<handler> mapped onto what an identifier can \
     carry, so names that differ only by the `Controller` suffix, by casing (`Posts`, \
     `PostsController`) or by a character that map replaces (`r#type`, `r_type`) publish one \
     id — which OpenAPI requires to be unique across the document";

fn contested_path_remedy() -> String {
    format!(
        "read the per-version document at /api-json/v{{n}}, or name the version /api-json \
         describes with {}",
        nest_rs_config::var_name("http", "DEFAULT_VERSION"),
    )
}

/// The RFC 9457 `application/problem+json` schema referenced by every error response.
///
/// `type` and `instance` are URI references (§3.1.1, §3.1.5): a relative one
/// such as the request path is valid, and `format: uri` would make a validating
/// client refuse it.
fn problem_details_schema() -> Value {
    json!({
        "type": "object",
        "description": "An RFC 9457 problem details object.",
        "properties": {
            "type": { "type": "string", "format": "uri-reference" },
            "title": { "type": "string" },
            "status": { "type": "integer", "minimum": 100, "maximum": 599 },
            "detail": { "type": "string" },
            "instance": { "type": "string", "format": "uri-reference" },
            "errors": { "type": "object", "additionalProperties": true },
        },
        "required": ["type", "title", "status"],
    })
}

fn route_is_guarded(route: &HttpRouteMeta, global_guards: bool) -> bool {
    (route.scoped_guarded || global_guards) && !route.public
}

fn operation_object(
    route: &HttpRouteMeta,
    full_path: &RouteTemplate,
    operation_id: &str,
    generator: &mut SchemaGenerator,
    global_guards: bool,
    version_parameter: Option<Value>,
) -> Value {
    let mut op = Map::new();
    op.insert("operationId".into(), json!(operation_id));
    op.insert("tags".into(), json!(route.tags));
    if let Some(summary) = route.summary {
        op.insert("summary".into(), json!(summary));
    }
    if let Some(description) = route.description {
        op.insert("description".into(), json!(description));
    }
    if route.deprecation.is_some() {
        op.insert("deprecated".into(), json!(true));
    }

    let mut parameters = typed_path_parameters(full_path, route.path_params, generator);
    parameters.extend(expand_object_params(route.query_params, "query", generator));
    parameters.extend(expand_object_params(
        route.header_params,
        "header",
        generator,
    ));
    parameters.extend(version_parameter);
    // A path parameter is exempt: omitting it reaches another route, never a `400`.
    let requires_parameter = parameters
        .iter()
        .any(|p| p["required"] == true && p["in"] != "path");
    if !parameters.is_empty() {
        op.insert("parameters".into(), Value::Array(parameters));
    }

    if let Some(body) = route.request_body {
        let schema = match body.schema() {
            Some(schema_fn) => schema_fn(generator).to_value(),
            // A bare `Multipart`: the media type is known, the parts are not.
            None => json!({ "type": "object", "additionalProperties": true }),
        };
        op.insert(
            "requestBody".into(),
            json!({ "required": true, "content": media_content(body.media_type(), schema) }),
        );
    }

    if route_is_guarded(route, global_guards) {
        op.insert("security".into(), json!([{ "bearerAuth": [] }]));
    } else if route.public {
        // OpenAPI 3.1 §4.8.10.1: an empty list removes any requirement, the
        // `#[public]` opening stated in the document as in the code.
        op.insert("security".into(), json!([]));
    }

    let mut responses = Map::new();
    let status = route.success_status;
    let mut ok = Map::new();
    let has_body = status != 204 && !is_redirect(status);
    let schema = has_body.then_some(route.response).flatten();
    ok.insert(
        "description".into(),
        json!(match (route.masked, schema.is_some()) {
            (true, true) => format!(
                "{} — field-level authorization applies: properties the caller's ability \
                 does not grant are omitted from the response.",
                reason_phrase(status),
            ),
            _ => reason_phrase(status).to_owned(),
        }),
    );
    let media = route.response_content_type.unwrap_or(JSON_MEDIA_TYPE);
    match (schema, route.response_content_type) {
        (Some(schema_fn), _) => {
            ok.insert(
                "content".into(),
                media_content(media, schema_fn(generator).to_value()),
            );
        }
        (None, Some(declared)) if has_body => {
            ok.insert(
                "content".into(),
                media_content(media, stream_schema(declared)),
            );
        }
        _ => {}
    }
    let headers = success_headers(route, status);
    if !headers.is_empty() {
        ok.insert("headers".into(), Value::Object(headers));
    }
    responses.insert(status.to_string(), Value::Object(ok));
    for (status, title) in error_statuses(route, full_path, global_guards, requires_parameter) {
        let mut response = problem_response(title);
        if let Value::Object(map) = &mut response {
            match status {
                "401" => {
                    map.insert("headers".into(), challenge_header());
                }
                "429" => {
                    map.insert("headers".into(), retry_after_header());
                }
                _ => {}
            }
        }
        responses.insert(status.into(), response);
    }
    // A body the handler writes itself sits beside the framework's problem
    // document for that status, when both can answer.
    for (status, schema_fn) in route.error_responses {
        let schema = schema_fn(generator).to_value();
        let response = responses
            .entry(status.to_string())
            .or_insert_with(|| json!({ "description": reason_phrase(*status) }));
        if let Value::Object(response) = response {
            let content = response
                .entry("content")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(content) = content {
                content.insert(JSON_MEDIA_TYPE.into(), json!({ "schema": schema }));
            }
        }
    }
    op.insert("responses".into(), Value::Object(responses));

    Value::Object(op)
}

/// Path parameters typed positionally from the handler's `Path<T>`, only when
/// every segment has one: a `Bind<_, _>` leaves fewer and would misalign.
fn typed_path_parameters(
    path: &RouteTemplate,
    path_params: &[nest_rs_http::__private::SchemaFn],
    generator: &mut SchemaGenerator,
) -> Vec<Value> {
    let names: Vec<&str> = path.parameters().collect();
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

/// Expand each payload struct into one parameter per property of its object
/// schema, filed under `location` (`query` or `header`).
fn expand_object_params(
    params: &[nest_rs_http::__private::SchemaFn],
    location: &str,
    generator: &mut SchemaGenerator,
) -> Vec<Value> {
    let mut out = Vec::new();
    for schema_fn in params {
        // The shared generator, or a nested type's `$ref` dangles.
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
                    "in": location,
                    "required": required.contains(&name.as_str()),
                    "schema": prop_schema,
                }));
            }
        }
    }
    out
}

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
fn error_statuses(
    route: &HttpRouteMeta,
    full_path: &RouteTemplate,
    global_guards: bool,
    requires_parameter: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if route.request_body.is_some() || requires_parameter {
        // Edge validation answers `400`, never `422` (`nest_rs_http::pipe::reject`).
        out.push(("400", "Bad Request"));
    }
    // The transport's body cap, whatever the media type.
    let content_too_large = route
        .request_body
        .is_some()
        .then_some(("413", "Content Too Large"));
    if route_is_guarded(route, global_guards) {
        out.push(("401", "Unauthorized"));
        out.push(("403", "Forbidden"));
    }
    // Off the path, not `path_params`: a `Bind<_, _>` route can 404 and has none.
    if full_path.parameters().next().is_some() {
        out.push(("404", "Not Found"));
    }
    if route.may_conflict {
        out.push(("409", "Conflict"));
    }
    out.extend(content_too_large);
    if route.throttled {
        out.push(("429", "Too Many Requests"));
    }
    // A handler's `Err` answers an opaque `500`; not `default`, which would
    // also claim a status a handler writes with a body of its own.
    out.push(("500", "Internal Server Error"));
    out
}

fn is_redirect(status: u16) -> bool {
    (300..400).contains(&status)
}

fn reason_phrase(status: u16) -> &'static str {
    StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Success")
}

const JSON_MEDIA_TYPE: &str = "application/json";

fn media_content(media_type: &str, schema: Value) -> Value {
    let mut content = Map::new();
    content.insert(media_type.to_owned(), json!({ "schema": schema }));
    Value::Object(content)
}

/// The body schema for a response known only by its media type; case-insensitive
/// per RFC 9110 §8.3.1.
fn stream_schema(media_type: &str) -> Value {
    let is_text = media_type
        .split('/')
        .next()
        .is_some_and(|top| top.trim().eq_ignore_ascii_case("text"));
    if is_text {
        json!({ "type": "string" })
    } else {
        json!({ "type": "string", "format": "binary" })
    }
}

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

/// The `WWW-Authenticate` challenge every `401` carries (RFC 9110 §15.5.2).
fn challenge_header() -> Value {
    json!({
        "WWW-Authenticate": {
            "description": "The challenge: `Bearer`, with RFC 6750's `error` when a credential \
                            arrived and was refused.",
            "required": true,
            "schema": { "type": "string" }
        }
    })
}

/// The `Retry-After` header a `429` carries, in seconds (RFC 9110 §10.2.3).
fn retry_after_header() -> Value {
    json!({
        "Retry-After": {
            "description": "Seconds to wait before retrying, until the rate-limit window resets.",
            "schema": { "type": "integer", "format": "int32", "minimum": 0 }
        }
    })
}

/// The headers the success response carries on the framework's behalf.
fn success_headers(route: &HttpRouteMeta, status: u16) -> Map<String, Value> {
    let mut headers = Map::new();
    if route.sets_location {
        headers.insert("Location".into(), location_header(status));
    }
    if route.sets_next_link {
        headers.insert(
            "Link".into(),
            json!({
                "description": "RFC 8288 `rel=\"next\"`: the next page, its `after` cursor \
                                carried in its query; absent on the last page.",
                "schema": { "type": "string" }
            }),
        );
    }
    if let Some(deprecation) = route.deprecation {
        headers.insert(
            "Deprecation".into(),
            json!({
                "description": format!("RFC 9745: deprecated since {}.", deprecation.since),
                "required": true,
                "schema": { "type": "string", "const": deprecation.header }
            }),
        );
    }
    for (name, value) in route.response_headers {
        // RFC 9110 §5.3: `Set-Cookie` alone is sent as several fields, so no
        // one value describes it.
        let several = name.eq_ignore_ascii_case("set-cookie")
            || route
                .response_headers
                .iter()
                .filter(|(n, _)| n == name)
                .count()
                > 1;
        let schema = match several {
            true => json!({ "type": "string" }),
            false => json!({ "type": "string", "const": value }),
        };
        headers.insert(
            (*name).into(),
            json!({ "required": true, "schema": schema }),
        );
    }
    headers
}

/// The `Location` header, an absolute-path reference (`uri-reference`). Not
/// `required`: a `#[crud]` create omits it for an entity not keyed on a `Uuid`.
fn location_header(status: u16) -> Value {
    let description = if is_redirect(status) {
        "URI to follow for this resource."
    } else {
        "URI of the resource that was just created."
    };
    json!({
        "description": description,
        "schema": { "type": "string", "format": "uri-reference" }
    })
}

/// Order two versions the way a reader does: unversioned below versioned, then
/// **naturally** — digit runs compared as numbers, so `2` < `9` < `10`.
fn compare_versions(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => natural_cmp(a, b),
    }
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    /// The leading run of digits without its leading zeros, and what follows it.
    fn digits(s: &[u8]) -> (&[u8], &[u8]) {
        let run = s.iter().take_while(|c| c.is_ascii_digit()).count();
        let (run, rest) = s.split_at(run);
        let significant = run.iter().position(|c| *c != b'0').unwrap_or(run.len());
        (&run[significant..], rest)
    }

    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let ((a_digits, a_rest), (b_digits, b_rest)) = (digits(a), digits(b));
                let ordering = a_digits
                    .len()
                    .cmp(&b_digits.len())
                    .then_with(|| a_digits.cmp(b_digits));
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
                (a, b) = (a_rest, b_rest);
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                (a, b) = (&a[1..], &b[1..]);
            }
        }
    }
}

/// The OpenAPI path template of `template`, which the route grammar already
/// spells (OpenAPI 3.1 §4.8.2), or `None` for what the standard cannot
/// template: a catch-all spans segments, and a literal brace reads as a
/// template expression.
fn openapi_path(template: &RouteTemplate) -> Option<&str> {
    let path = template.as_str();
    // A template doubles a brace only to write it literally.
    match template.catch_all().is_some() || path.contains("{{") || path.contains("}}") {
        true => None,
        false => Some(path),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_http::{DEFAULT_VERSION_HEADER, HttpVerb, RequestBodyMeta};
    use schemars::JsonSchema;
    use schemars::generate::SchemaSettings;
    use serde::Serialize;

    use super::*;

    fn template(path: &str) -> RouteTemplate {
        RouteTemplate::parse(path).expect("a route template")
    }

    #[test]
    fn the_document_spells_a_template_as_the_route_declares_it() {
        for path in [
            "/",
            "/users",
            "/users/{id}",
            "/orgs/{org_id}/users/{id}",
            "/users/@{handle}",
        ] {
            assert_eq!(openapi_path(&template(path)), Some(path));
        }
    }

    #[test]
    fn openapi_path_refuses_what_the_standard_cannot_template() {
        // A catch-all spans segments; a literal brace reads as a template.
        for path in ["/blobs/{*rest}", "/b/{{x}}"] {
            assert!(
                openapi_path(&template(path)).is_none(),
                "{path} has no OpenAPI path template",
            );
        }
    }

    #[test]
    fn versions_order_the_way_they_read() {
        use std::cmp::Ordering;

        assert_eq!(compare_versions(Some("9"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("2"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("10"), Some("10")), Ordering::Equal);
        assert_eq!(compare_versions(None, Some("1")), Ordering::Less);
        assert_eq!(compare_versions(None, None), Ordering::Equal);
        assert_eq!(
            compare_versions(Some("2024-08-11"), Some("2024-09-01")),
            Ordering::Less,
        );
        assert_eq!(
            compare_versions(Some("2024-09-01"), Some("2025-01-01")),
            Ordering::Less,
        );
        assert_eq!(compare_versions(Some("007"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("01"), Some("1")), Ordering::Equal);
        let mut all = [Some("10"), None, Some("9"), Some("1"), Some("2")];
        all.sort_by(|a, b| compare_versions(*a, *b));
        assert_eq!(all, [None, Some("1"), Some("2"), Some("9"), Some("10")]);
    }

    #[test]
    fn derives_path_parameters() {
        let mut g = generator();
        let params = typed_path_parameters(&template("/users/{id}"), &[], &mut g);
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
        assert!(typed_path_parameters(&template("/health"), &[], &mut g).is_empty());
        assert!(typed_path_parameters(&template("/"), &[], &mut g).is_empty());
    }

    #[test]
    fn path_parameters_emits_one_object_per_segment() {
        let mut g = generator();
        let params = typed_path_parameters(&template("/orgs/{org_id}/users/{id}"), &[], &mut g);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["name"], "org_id");
        assert_eq!(params[1]["name"], "id");
    }

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

    #[derive(Serialize, JsonSchema)]
    struct OptionalHeader {
        #[serde(rename = "Last-Event-ID")]
        last_event_id: Option<u32>,
    }

    fn schema_for_optional(generator: &mut SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<OptionalHeader>()
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
            response_content_type: None,
            error_responses: &[],
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            sets_next_link: false,
            response_headers: &[],
            success_status: 200,
            scoped_guarded: false,
            public: false,
            deprecation: None,
            versions: &[],
        }
    }

    const HOST_TOKEN: &str = "test";

    fn operation(
        route: &HttpRouteMeta,
        full_path: &str,
        generator: &mut SchemaGenerator,
        global_guards: bool,
        version_parameter: Option<Value>,
    ) -> Value {
        operation_object(
            route,
            &template(full_path),
            &operation_id(HOST_TOKEN, route.handler, None),
            generator,
            global_guards,
            version_parameter,
        )
    }

    #[test]
    fn a_path_param_segment_advertises_404_but_a_literal_segment_does_not() {
        let bound = error_statuses(
            &route("get_user", "/users/{id}"),
            &template("/users/{id}"),
            false,
            false,
        );
        assert!(
            bound.iter().any(|(s, _)| *s == "404"),
            "an `{{id}}` route advertises 404",
        );

        let literal = error_statuses(
            &route("weird", "/a-b/list"),
            &template("/a-b/list"),
            false,
            false,
        );
        assert!(
            !literal.iter().any(|(s, _)| *s == "404"),
            "a literal segment must not advertise 404",
        );
    }

    #[test]
    fn a_throttled_route_advertises_429_with_a_retry_after_header() {
        let mut g = generator();
        let mut r = route("upload", "/audio/uploads");
        r.throttled = true;
        let op = operation(&r, "/audio/uploads", &mut g, false, None);
        let too_many = &op["responses"]["429"];
        assert_eq!(too_many["description"], "Too Many Requests");
        assert!(
            too_many["headers"]["Retry-After"]["schema"]["type"] == "integer",
            "429 must document the Retry-After header: {too_many}",
        );
    }

    #[test]
    fn a_create_route_declares_the_location_header_on_its_201() {
        let mut g = generator();
        let mut r = route("create", "/orgs");
        r.verb = HttpVerb::Post;
        r.success_status = 201;
        r.sets_location = true;
        let op = operation(&r, "/orgs", &mut g, false, None);
        let created = &op["responses"]["201"];
        assert_eq!(created["description"], "Created");
        assert_eq!(
            created["headers"]["Location"]["schema"]["format"], "uri-reference",
            "201 must document the Location header: {created}",
        );
        assert!(
            created["headers"]["Location"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("created")),
            "the description names what the URI points at: {created}",
        );
    }

    #[test]
    fn a_redirect_declares_the_location_it_sends() {
        let mut g = generator();
        let mut r = route("legacy", "/old");
        r.success_status = 301;
        r.sets_location = true;
        let op = operation(&r, "/old", &mut g, false, None);
        let moved = &op["responses"]["301"];
        assert_eq!(
            moved["headers"]["Location"]["schema"]["format"], "uri-reference",
            "a redirect documents the header it is built around: {moved}",
        );
        assert!(
            moved["headers"]["Location"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("follow")),
            "the description names a target, not a created row: {moved}",
        );
    }

    #[test]
    fn a_route_that_sends_no_location_declares_none() {
        let mut g = generator();
        let op = operation(&route("list", "/orgs"), "/orgs", &mut g, false, None);
        assert!(
            op["responses"]["200"].get("headers").is_none(),
            "a route with no Location must not declare one: {op}",
        );
    }

    #[test]
    fn an_unthrottled_route_does_not_advertise_429() {
        let statuses = error_statuses(&route("list", "/audio"), &template("/audio"), false, false);
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
        let op = operation(&r, "/health", &mut g, false, None);
        assert_eq!(op["operationId"], "test_get_health");
        assert_eq!(op["tags"][0], "health");
    }

    #[test]
    fn operation_object_skips_optional_metadata_when_absent() {
        let mut g = generator();
        let op = operation(&route("h", "/h"), "/h", &mut g, false, None);
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
        let op = operation(&r, "/h", &mut g, false, None);
        assert_eq!(op["summary"], "Quick");
        assert_eq!(op["description"], "Full prose");
    }

    #[test]
    fn operation_object_inlines_parameters_when_path_has_any() {
        let mut g = generator();
        let r = route("get_user", "/users/{id}");
        let op = operation(&r, "/users/{id}", &mut g, false, None);
        assert!(op["parameters"].is_array());
        assert_eq!(op["parameters"][0]["name"], "id");
    }

    #[test]
    fn operation_object_attaches_request_body_when_a_schema_fn_is_present() {
        let mut g = generator();
        let mut r = route("create_user", "/users");
        r.request_body = Some(RequestBodyMeta::Json(schema_for_dummy));
        let op = operation(&r, "/users", &mut g, false, None);
        assert_eq!(op["requestBody"]["required"], true);
        assert!(op["requestBody"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn a_multipart_body_is_documented_under_its_own_media_type() {
        let mut g = generator();
        let mut r = route("upload", "/audio/uploads/direct");
        r.verb = HttpVerb::Post;
        r.request_body = Some(RequestBodyMeta::Multipart(Some(schema_for_dummy)));
        let op = operation(&r, "/audio/uploads/direct", &mut g, false, None);
        assert!(
            op["requestBody"]["content"]["multipart/form-data"]["schema"].is_object(),
            "the parts are typed under multipart/form-data: {op}",
        );
        assert!(
            op["requestBody"]["content"]
                .get("application/json")
                .is_none(),
            "and not under JSON: {op}",
        );
    }

    #[test]
    fn an_untyped_multipart_body_still_declares_its_media_type() {
        let mut g = generator();
        let mut r = route("upload", "/uploads");
        r.verb = HttpVerb::Post;
        r.request_body = Some(RequestBodyMeta::Multipart(None));
        let op = operation(&r, "/uploads", &mut g, false, None);
        let schema = &op["requestBody"]["content"]["multipart/form-data"]["schema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], true);
        assert!(
            op["responses"].get("400").is_some(),
            "a body is a 400 the route can produce, whatever media type it is",
        );
    }

    #[test]
    fn a_declared_response_media_type_carries_a_binary_body_schema() {
        let mut g = generator();
        let mut r = route("download", "/audio/download");
        r.response_content_type = Some("audio/mpeg");
        let op = operation(&r, "/audio/download", &mut g, false, None);
        let content = &op["responses"]["200"]["content"];
        assert_eq!(content["audio/mpeg"]["schema"]["type"], "string");
        assert_eq!(content["audio/mpeg"]["schema"]["format"], "binary");
        assert!(
            content.get("application/json").is_none(),
            "the declared media type replaces the JSON default: {op}",
        );
    }

    #[test]
    fn a_text_stream_is_typed_string_without_a_binary_format() {
        let mut g = generator();
        let mut r = route("events", "/audio/events");
        r.response_content_type = Some("text/event-stream");
        let op = operation(&r, "/audio/events", &mut g, false, None);
        let schema = &op["responses"]["200"]["content"]["text/event-stream"]["schema"];
        assert_eq!(schema["type"], "string");
        assert!(schema.get("format").is_none(), "{schema}");
    }

    #[test]
    fn a_declared_media_type_files_a_declared_response_schema_under_itself() {
        let mut g = generator();
        let mut r = route("export", "/exports");
        r.response = Some(schema_for_dummy);
        r.response_content_type = Some("application/x-ndjson");
        let op = operation(&r, "/exports", &mut g, false, None);
        assert!(op["responses"]["200"]["content"]["application/x-ndjson"]["schema"].is_object());
    }

    #[test]
    fn a_bodyless_success_declares_no_streamed_content() {
        let mut g = generator();
        let mut r = route("purge", "/exports");
        r.success_status = 204;
        r.response_content_type = Some("application/octet-stream");
        let op = operation(&r, "/exports", &mut g, false, None);
        assert!(op["responses"]["204"].get("content").is_none());
    }

    #[test]
    fn header_payloads_expand_into_in_header_parameters() {
        let mut g = generator();
        let mut r = route("list", "/things");
        r.header_params = &[schema_for_dummy];
        let op = operation(&r, "/things", &mut g, false, None);
        let params = op["parameters"].as_array().expect("parameters");
        let header: Vec<&str> = params
            .iter()
            .filter(|p| p["in"] == "header")
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert_eq!(header, ["name"], "one parameter per property: {op}");
        assert_eq!(params[0]["required"], true, "`name` is a required property");
        assert!(
            op["responses"].get("400").is_some(),
            "a required header is a 400 this operation can produce: {op}",
        );
    }

    #[test]
    fn an_optional_header_does_not_advertise_a_400() {
        let mut g = generator();
        let mut r = route("events", "/audio/events");
        r.header_params = &[schema_for_optional];
        let op = operation(&r, "/audio/events", &mut g, false, None);
        assert_eq!(op["parameters"][0]["in"], "header");
        assert_eq!(op["parameters"][0]["required"], false);
        assert!(op["responses"].get("400").is_none(), "{op}");
    }

    #[test]
    fn query_and_header_parameters_coexist_on_one_operation() {
        let mut g = generator();
        let mut r = route("events", "/audio/events");
        r.query_params = &[schema_for_dummy];
        r.header_params = &[schema_for_optional];
        let op = operation(&r, "/audio/events", &mut g, false, None);
        let locations: Vec<&str> = op["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .filter_map(|p| p["in"].as_str())
            .collect();
        assert_eq!(locations, ["query", "header"]);
    }

    #[test]
    fn operation_object_always_emits_a_200_response_with_description() {
        let mut g = generator();
        let op = operation(&route("h", "/h"), "/h", &mut g, false, None);
        assert_eq!(op["responses"]["200"]["description"], "OK");
        assert!(op["responses"]["200"].get("content").is_none());
    }

    #[test]
    fn operation_object_attaches_response_schema_when_present() {
        let mut g = generator();
        let mut r = route("get_user", "/users/{id}");
        r.response = Some(schema_for_dummy);
        let op = operation(&r, "/users/{id}", &mut g, false, None);
        assert!(op["responses"]["200"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn a_masked_route_publishes_its_schema_and_flags_the_field_set() {
        let mut g = generator();
        let mut r = route("list_users", "/users");
        r.response = Some(schema_for_dummy);
        r.masked = true;
        let op = operation(&r, "/users", &mut g, false, None);
        assert!(
            op["responses"]["200"]["content"]["application/json"]["schema"].is_object(),
            "the shape is published: {op}",
        );
        let description = op["responses"]["200"]["description"]
            .as_str()
            .expect("a description");
        assert!(description.starts_with("OK"), "{description}");
        assert!(
            description.contains("ability"),
            "and it says the fields depend on the caller: {description}",
        );
    }

    #[test]
    fn a_masked_bodyless_response_keeps_the_plain_description() {
        let mut g = generator();
        let mut r = route("delete_user", "/users/{id}");
        r.response = Some(schema_for_dummy);
        r.masked = true;
        r.success_status = 204;
        let op = operation(&r, "/users/{id}", &mut g, false, None);
        assert_eq!(op["responses"]["204"]["description"], "No Content");
        assert!(op["responses"]["204"].get("content").is_none());
    }

    #[test]
    fn a_non_200_success_status_replaces_the_200_response() {
        let mut g = generator();
        let mut r = route("create_user", "/users");
        r.success_status = 201;
        r.response = Some(schema_for_dummy);
        let op = operation(&r, "/users", &mut g, false, None);
        assert!(op["responses"].get("200").is_none(), "no bogus 200");
        assert_eq!(op["responses"]["201"]["description"], "Created");
        assert!(op["responses"]["201"]["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn a_204_or_redirect_success_carries_no_body() {
        for (status, reason) in [(204, "No Content"), (307, "Temporary Redirect")] {
            let mut g = generator();
            let mut r = route("delete_user", "/users/{id}");
            r.success_status = status;
            r.response = Some(schema_for_dummy);
            let op = operation(&r, "/users/{id}", &mut g, false, None);
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
        let mut g = generator();
        let r = route("list", "/users");
        let op = operation(&r, "/users", &mut g, true, None);
        assert_eq!(op["security"][0]["bearerAuth"], json!([]));
        assert_eq!(
            op["responses"]["401"]["headers"]["WWW-Authenticate"]["required"], true,
            "a 401 names its challenge: {op}",
        );
        assert!(op["responses"].get("403").is_some());
    }

    #[test]
    fn a_public_route_stays_unsecured_even_under_a_global_guard_pool() {
        let mut g = generator();
        let mut r = route("health", "/health");
        r.public = true;
        let op = operation(&r, "/health", &mut g, true, None);
        assert_eq!(
            op["security"],
            json!([]),
            "the opening is stated, not inferred from an absence: {op}",
        );
        assert!(op["responses"].get("401").is_none());
    }

    #[test]
    fn an_implicit_route_claims_no_posture_in_the_document() {
        let mut g = generator();
        let op = operation(&route("list", "/things"), "/things", &mut g, false, None);
        assert!(
            op.get("security").is_none(),
            "neither guarded nor `#[public]`: the document states nothing the code does not: {op}",
        );
    }

    #[test]
    fn every_operation_types_the_opaque_500_as_problem_details() {
        let mut g = generator();
        let op = operation(&route("h", "/h"), "/h", &mut g, false, None);
        assert_eq!(
            op["responses"]["500"]["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDetails",
            "{op}",
        );
        assert!(
            op["responses"].get("413").is_none(),
            "no body, no cap: {op}"
        );
    }

    #[test]
    fn a_route_taking_a_body_advertises_the_cap_it_can_hit() {
        let mut g = generator();
        let mut r = route("upload", "/uploads");
        r.verb = HttpVerb::Post;
        r.request_body = Some(RequestBodyMeta::Multipart(None));
        let op = operation(&r, "/uploads", &mut g, false, None);
        assert_eq!(
            op["responses"]["413"]["description"], "Content Too Large",
            "{op}"
        );
    }

    #[test]
    fn a_deprecated_route_is_marked_and_documents_its_header() {
        let mut g = generator();
        let mut r = route("list", "/v1/posts");
        r.deprecation = Some(nest_rs_http::DeprecationMeta {
            since: "2026-10-08",
            header: "@1791417600",
        });
        let op = operation(&r, "/v1/posts", &mut g, false, None);
        assert_eq!(op["deprecated"], true, "{op}");
        let header = &op["responses"]["200"]["headers"]["Deprecation"];
        assert_eq!(header["schema"]["const"], "@1791417600", "{op}");
        assert!(
            header["description"]
                .as_str()
                .is_some_and(|d| d.contains("2026-10-08")),
            "{header}",
        );
        let current = operation(
            &route("list", "/v2/posts"),
            "/v2/posts",
            &mut g,
            false,
            None,
        );
        assert!(current.get("deprecated").is_none());
    }

    #[test]
    fn a_declared_error_body_is_documented_under_its_status() {
        let mut g = generator();
        let mut r = route("ready", "/health/ready");
        r.error_responses = &[(503, schema_for_dummy)];
        let op = operation(&r, "/health/ready", &mut g, false, None);
        let unavailable = &op["responses"]["503"];
        assert_eq!(unavailable["description"], "Service Unavailable", "{op}");
        assert!(
            unavailable["content"]["application/json"]["schema"].is_object(),
            "{op}"
        );
        assert!(
            unavailable["content"]
                .get("application/problem+json")
                .is_none()
        );
    }

    #[test]
    fn a_declared_error_body_sits_beside_the_frameworks_problem_document() {
        let mut g = generator();
        let mut r = route("get", "/users/{id}");
        r.error_responses = &[(404, schema_for_dummy)];
        let op = operation(&r, "/users/{id}", &mut g, false, None);
        let content = &op["responses"]["404"]["content"];
        assert!(content["application/problem+json"].is_object(), "{op}");
        assert!(content["application/json"].is_object(), "{op}");
    }

    #[test]
    fn a_problem_points_at_uri_references_so_a_relative_instance_validates() {
        let schema = problem_details_schema();
        assert_eq!(schema["properties"]["type"]["format"], "uri-reference");
        assert_eq!(schema["properties"]["instance"]["format"], "uri-reference");
    }

    #[test]
    fn a_paginated_list_declares_the_link_it_sends() {
        let mut g = generator();
        let mut r = route("list", "/posts");
        r.sets_next_link = true;
        let op = operation(&r, "/posts", &mut g, false, None);
        let header = &op["responses"]["200"]["headers"]["Link"];
        assert_eq!(header["schema"]["type"], "string", "{op}");
        assert!(
            header.get("required").is_none(),
            "the last page sends none: {header}",
        );
    }

    #[test]
    fn a_declared_response_header_is_documented_with_its_value() {
        let mut g = generator();
        let mut r = route("feed", "/feed");
        r.response_headers = &[("cache-control", "no-store")];
        let op = operation(&r, "/feed", &mut g, false, None);
        let header = &op["responses"]["200"]["headers"]["cache-control"];
        assert_eq!(header["required"], true, "{op}");
        assert_eq!(header["schema"]["const"], "no-store", "{op}");
    }

    #[test]
    fn a_header_sent_as_several_fields_claims_no_single_value() {
        let mut g = generator();
        let mut r = route("login", "/login");
        r.response_headers = &[("set-cookie", "a=1"), ("x-tier", "a"), ("x-tier", "b")];
        let op = operation(&r, "/login", &mut g, false, None);
        let headers = &op["responses"]["200"]["headers"];
        assert!(
            headers["set-cookie"]["schema"].get("const").is_none(),
            "{headers}"
        );
        assert!(
            headers["x-tier"]["schema"].get("const").is_none(),
            "{headers}"
        );
    }

    fn info(title: &str, version: &str, description: Option<&str>) -> OpenApiConfig {
        OpenApiConfig {
            title: title.to_owned(),
            version: version.to_owned(),
            description: description.map(str::to_owned),
            ..OpenApiConfig::default()
        }
    }

    #[test]
    fn build_document_emits_openapi_3_1_with_info_and_no_paths_for_empty_discovery() {
        let container = Container::builder().build();
        let doc = build_document(
            &container,
            &info("Test API", "1.2.3", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(doc["openapi"], "3.1.2");
        assert_eq!(doc["info"]["title"], "Test API");
        assert_eq!(doc["info"]["version"], "1.2.3");
        assert!(doc["info"].get("description").is_none());
        assert!(doc["paths"].is_object());
        assert!(doc["components"]["schemas"].is_object());
    }

    #[test]
    fn a_query_payload_leaves_no_orphan_component_but_a_body_keeps_its_own() {
        let mut search = route("search", "/");
        search.query_params = &[schema_for_dummy];
        let mut submit = route("submit", "/");
        submit.verb = HttpVerb::Post;
        submit.request_body = Some(RequestBodyMeta::Json(schema_for_optional));
        let container = deployment(vec![controller(
            "ThingsController",
            "things",
            "/things",
            &[],
            vec![search],
        )]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert!(
            doc["components"]["schemas"].get("DummyBody").is_none(),
            "a query payload is expanded into parameters, its definition reached by nothing: {doc}",
        );

        let container = deployment(vec![controller(
            "ThingsController",
            "things",
            "/things",
            &[],
            vec![submit],
        )]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert!(
            doc["components"]["schemas"].get("OptionalHeader").is_some(),
            "a body's component is referenced and stays: {doc}",
        );
        assert!(doc["components"]["schemas"].get("ProblemDetails").is_some());
    }

    #[test]
    fn build_document_carries_description_when_supplied() {
        let container = Container::builder().build();
        let doc = build_document(
            &container,
            &info("X", "0", Some("a description")),
            None,
            &mut Reported::default(),
        );
        assert_eq!(doc["info"]["description"], "a description");
    }

    #[test]
    fn the_info_object_carries_every_field_the_config_sets() {
        let config = OpenApiConfig {
            summary: Some("Publish's REST surface".into()),
            terms_of_service: Some("https://publish.example/terms".into()),
            contact: Some(crate::OpenApiContact {
                name: Some("API team".into()),
                url: None,
                email: Some("api@publish.example".into()),
            }),
            license: Some(crate::OpenApiLicense {
                name: "Apache 2.0".into(),
                identifier: Some("Apache-2.0".into()),
                url: None,
            }),
            ..info("Publish", "2.0.0", None)
        };
        let info = info_object(&config);
        assert_eq!(info["summary"], "Publish's REST surface");
        assert_eq!(info["termsOfService"], "https://publish.example/terms");
        assert_eq!(
            info["contact"],
            json!({ "name": "API team", "email": "api@publish.example" })
        );
        assert_eq!(
            info["license"],
            json!({ "name": "Apache 2.0", "identifier": "Apache-2.0" })
        );
        assert!(
            info_object(&OpenApiConfig::default())
                .get("license")
                .is_none()
        );
    }

    #[test]
    fn the_server_is_the_origin_root_without_a_global_prefix() {
        let container = Container::builder().build();
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(doc["servers"], json!([{ "url": "/" }]));
    }

    #[test]
    fn global_prefix_is_declared_as_a_normalized_server_base_url() {
        for raw in ["api", "/api", "api/", "/api/"] {
            let container = Container::builder()
                .provide(HttpConfig::default().with_global_prefix(raw))
                .build();
            let doc = build_document(
                &container,
                &info("X", "0", None),
                None,
                &mut Reported::default(),
            );
            assert_eq!(
                doc["servers"][0]["url"], "/api",
                "prefix {raw:?} must normalize to `/api`",
            );
        }
    }

    fn selection(
        versioning: ApiVersioning,
        default_version: Option<&str>,
        claims: Option<&str>,
    ) -> Option<VersionSelection> {
        let container = Container::builder()
            .provide(HttpConfig {
                versioning,
                default_version: default_version.map(str::to_owned),
                ..HttpConfig::default()
            })
            .build();
        VersionSelection::resolve(&container, claims)
    }

    #[test]
    fn the_uri_strategy_produces_no_version_selection_at_all() {
        assert!(selection(ApiVersioning::Uri, Some("1"), Some("1")).is_none());
        assert!(selection(ApiVersioning::Uri, None, None).is_none());
    }

    #[test]
    fn the_version_header_is_required_only_where_no_default_answers_for_it() {
        let stated = selection(ApiVersioning::Header, None, None).expect("a selection");
        assert_eq!(
            stated.parameter("2", &route("list", "/posts"))["required"],
            true,
            "with no default version the caller must state one",
        );
        let defaulted = selection(ApiVersioning::Header, Some("1"), None).expect("a selection");
        assert_eq!(
            defaulted.parameter("1", &route("list", "/posts"))["required"],
            false,
            "a default version answers for a caller that states none",
        );
    }

    #[test]
    fn the_header_strategy_documents_the_configured_header_and_its_versions() {
        let selection = selection(ApiVersioning::Header, None, None).expect("a selection");
        let parameter = selection.parameter("2", &route("list", "/posts"));
        assert_eq!(parameter["name"], DEFAULT_VERSION_HEADER);
        assert_eq!(parameter["in"], "header");
        assert_eq!(
            parameter["schema"]["enum"],
            json!(["2"]),
            "the schema enumerates what this document serves: {parameter}",
        );
    }

    #[test]
    fn the_media_type_strategy_documents_the_accept_header_it_reads() {
        let selection = selection(ApiVersioning::MediaType, None, None).expect("a selection");
        let parameter = selection.parameter("2", &route("list", "/posts"));
        assert_eq!(parameter["name"], "accept");
        assert_eq!(
            parameter["schema"]["enum"],
            json!(["application/json; version=2"])
        );
        assert!(
            parameter["description"]
                .as_str()
                .is_some_and(|d| d.contains(MEDIA_TYPE_PARAM)),
            "the description names the parameter a caller writes: {parameter}",
        );

        let mut streamed = route("events", "/audio/events");
        streamed.response_content_type = Some("text/event-stream");
        assert_eq!(
            selection.parameter("2", &streamed)["schema"]["enum"],
            json!(["text/event-stream; version=2"]),
        );
    }

    #[test]
    fn a_document_describes_its_own_version_and_every_unversioned_operation() {
        let claiming = selection(ApiVersioning::Header, None, Some("2")).expect("a selection");
        assert!(claiming.describes(Some("2")));
        assert!(
            !claiming.describes(Some("1")),
            "an operation from another version never appears in a document that does not claim it",
        );
        assert!(
            claiming.describes(None),
            "an unversioned controller carries no version parameter, so it belongs everywhere",
        );

        let default = selection(ApiVersioning::Header, Some("1"), None).expect("a selection");
        assert!(default.describes(Some("1")));
        assert!(!default.describes(Some("2")));

        let all = selection(ApiVersioning::Header, None, None).expect("a selection");
        assert!(all.describes(Some("1")) && all.describes(Some("2")));
    }

    #[test]
    fn a_versioned_operation_advertises_the_400_its_version_token_can_produce() {
        let mut g = generator();
        let selection = selection(ApiVersioning::Header, None, None).expect("a selection");
        let r = route("list", "/posts");
        let op = operation(
            &r,
            "/posts",
            &mut g,
            false,
            Some(selection.parameter("2", &r)),
        );
        assert_eq!(op["parameters"][0]["name"], DEFAULT_VERSION_HEADER);
        assert!(op["responses"].get("400").is_some(), "{op}");
    }

    #[test]
    fn a_required_query_property_advertises_the_400_it_produces() {
        let mut g = generator();
        let mut r = route("search", "/posts");
        r.query_params = &[schema_for_dummy];
        let op = operation(&r, "/posts", &mut g, false, None);
        assert_eq!(op["parameters"][0]["in"], "query");
        assert_eq!(op["parameters"][0]["required"], true);
        assert!(
            op["responses"].get("400").is_some(),
            "a required query property is a 400 this operation can produce: {op}",
        );
    }

    #[test]
    fn an_optional_query_property_advertises_no_400() {
        let mut g = generator();
        let mut r = route("list", "/posts");
        r.query_params = &[schema_for_optional];
        let op = operation(&r, "/posts", &mut g, false, None);
        assert_eq!(op["parameters"][0]["required"], false);
        assert!(op["responses"].get("400").is_none(), "{op}");
    }

    #[test]
    fn a_path_parameter_alone_advertises_no_400() {
        let mut g = generator();
        let op = operation(
            &route("get", "/users/{id}"),
            "/users/{id}",
            &mut g,
            false,
            None,
        );
        assert_eq!(op["parameters"][0]["in"], "path");
        assert_eq!(op["parameters"][0]["required"], true);
        assert!(op["responses"].get("400").is_none(), "{op}");
    }

    #[test]
    fn a_version_is_labelled_for_a_log_field() {
        assert_eq!(version_label(Some("2")), "2");
        assert_eq!(version_label(None), "unversioned");
    }

    #[test]
    fn the_contested_path_remedy_names_the_variable_and_the_other_document() {
        let remedy = contested_path_remedy();
        assert!(remedy.contains("DEFAULT_VERSION"), "{remedy}");
        assert!(remedy.contains("/api-json/v"), "{remedy}");
    }

    fn controller(
        name: &'static str,
        token: &'static str,
        path: &'static str,
        versions: &'static [&'static str],
        routes: Vec<HttpRouteMeta>,
    ) -> HttpControllerMeta {
        HttpControllerMeta::new(name, token, path, versions, routes, |_, route| route)
    }

    fn crud_routes() -> Vec<HttpRouteMeta> {
        let mut list = route("list", "/");
        let mut get = route("get", "/{id}");
        let mut create = route("create", "/");
        create.verb = HttpVerb::Post;
        let mut update = route("update", "/{id}");
        update.verb = HttpVerb::Patch;
        let mut delete = route("delete", "/{id}");
        delete.verb = HttpVerb::Delete;
        list.tags = &["crud"];
        get.tags = &["crud"];
        vec![list, get, create, update, delete]
    }

    fn deployment(controllers: Vec<HttpControllerMeta>) -> Container {
        controllers
            .into_iter()
            .fold(Container::builder(), |builder, meta| {
                builder.provide_meta(meta)
            })
            .build()
    }

    fn id_at<'a>(document: &'a Value, path: &str, method: &str) -> Option<&'a str> {
        document["paths"][path][method]["operationId"].as_str()
    }

    fn ids(document: &Value) -> Vec<&str> {
        document["paths"]
            .as_object()
            .expect("paths")
            .values()
            .filter_map(Value::as_object)
            .flat_map(|methods| methods.values())
            .filter_map(|op| op["operationId"].as_str())
            .collect()
    }

    #[test]
    fn two_crud_shaped_controllers_publish_no_duplicate_id() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = deployment(vec![
            controller("PostsController", "posts", "/posts", &[], crud_routes()),
            controller("UsersController", "users", "/users", &[], crud_routes()),
        ]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );

        let published = ids(&doc);
        let mut unique = published.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            published.len(),
            "one id per operation: {published:?}",
        );
        assert_eq!(id_at(&doc, "/posts", "get"), Some("posts_list"));
        assert_eq!(id_at(&doc, "/users", "get"), Some("users_list"));
        assert_eq!(id_at(&doc, "/posts/{id}", "patch"), Some("posts_update"));
        assert!(
            logs.find("nest_rs::openapi", "two operations share one operationId")
                .is_empty(),
            "and nothing to warn about: {:#?}",
            logs.events(),
        );
    }

    #[test]
    fn one_handler_mounted_under_two_versions_gets_two_operation_ids() {
        let container = deployment(vec![controller(
            "ReportsController",
            "reports",
            "/reports",
            &["1", "2"],
            vec![route("list", "/")],
        )]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(id_at(&doc, "/v1/reports", "get"), Some("reports_list_v1"));
        assert_eq!(id_at(&doc, "/v2/reports", "get"), Some("reports_list_v2"));
    }

    #[test]
    fn a_path_two_versions_contest_names_the_one_the_default_document_drops() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = Container::builder()
            .provide(HttpConfig {
                versioning: ApiVersioning::Header,
                ..HttpConfig::default()
            })
            .provide_meta(controller(
                "PostsV1Controller",
                "posts",
                "/posts",
                &["1"],
                vec![route("list", "/")],
            ))
            .provide_meta(controller(
                "PostsV2Controller",
                "posts",
                "/posts",
                &["2"],
                vec![route("list", "/")],
            ))
            .build();
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(id_at(&doc, "/posts", "get"), Some("posts_list_v2"));

        let event = logs.expect_one(
            "nest_rs::openapi",
            "two API versions serve one documented path",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("path").as_deref(), Some("/posts"));
        assert_eq!(event.field("method").as_deref(), Some("GET"));
        assert_eq!(event.field("described").as_deref(), Some("2"));
        assert_eq!(event.field("omitted").as_deref(), Some("1"));
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("/api-json/v")),
            "the event carries the per-version document the omitted operation \
             is still served by, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn two_versions_of_different_paths_contest_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = Container::builder()
            .provide(HttpConfig {
                versioning: ApiVersioning::Header,
                ..HttpConfig::default()
            })
            .provide_meta(controller(
                "PostsController",
                "posts",
                "/posts",
                &["1"],
                vec![route("list", "/")],
            ))
            .provide_meta(controller(
                "UsersController",
                "users",
                "/users",
                &["2"],
                vec![route("list", "/")],
            ))
            .build();
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(id_at(&doc, "/posts", "get"), Some("posts_list_v1"));
        assert_eq!(id_at(&doc, "/users", "get"), Some("users_list_v2"));
        logs.expect_none(
            "nest_rs::openapi",
            "two API versions serve one documented path",
        );
    }

    #[test]
    fn an_id_is_the_controller_token_and_the_handler() {
        assert_eq!(operation_id("posts", "list", None), "posts_list");
        assert_eq!(
            operation_id("audio_uploads", "get", None),
            "audio_uploads_get"
        );
    }

    #[test]
    fn a_raw_ident_handler_reaches_the_document_as_an_identifier() {
        let id = operation_id("probe", "r#type", None);
        assert_eq!(id, "probe_r_type");
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "an operationId a generator can name a method after: {id}",
        );
    }

    #[test]
    fn a_raw_ident_controller_is_mapped_by_the_same_rule() {
        assert_eq!(operation_id("r#_type", "list", None), "r__type_list");
        assert_eq!(
            operation_id("r#_type", "r#type", Some("2024-08-11")),
            "r__type_r_type_v2024_08_11",
        );
    }

    #[test]
    fn two_handler_spellings_that_map_to_one_id_are_reported_by_the_ledger() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = deployment(vec![controller(
            "ProbeController",
            "probe",
            "/probe",
            &[],
            vec![route("r#type", "/raw"), route("r_type", "/plain")],
        )]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );

        let event = logs.expect_one("nest_rs::openapi", "two operations share one operationId");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("operation_id").as_deref(), Some("probe_r_type"));
        let addresses = [event.field("path"), event.field("conflicting_path")];
        assert!(
            addresses.iter().flatten().any(|p| p == "/probe/raw")
                && addresses.iter().flatten().any(|p| p == "/probe/plain"),
            "the warning names both operations: {event:#?}",
        );
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("one of the two handlers")),
            "and the remedy covers the half that produced this one: {event:#?}",
        );

        assert!(
            id_at(&doc, "/probe/raw", "get").is_some()
                && id_at(&doc, "/probe/plain", "get").is_some()
        );
    }

    #[test]
    fn a_version_that_is_not_a_bare_integer_still_reads_as_an_identifier() {
        let id = operation_id("reports", "list", Some("2024-08-11"));
        assert_eq!(id, "reports_list_v2024_08_11");
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "an operationId a generator can name a method after: {id}",
        );

        let container = deployment(vec![controller(
            "ReportsController",
            "reports",
            "/reports",
            &["2024-08-11"],
            vec![route("list", "/")],
        )]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(id_at(&doc, "/v2024-08-11/reports", "get"), Some(&*id));
    }

    #[test]
    fn two_controller_names_that_reduce_to_one_token_are_both_named_in_a_warning() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = deployment(vec![
            controller(
                "PostsController",
                "posts",
                "/posts",
                &[],
                vec![route("list", "/")],
            ),
            controller(
                "Posts",
                "posts",
                "/archive/posts",
                &[],
                vec![route("list", "/")],
            ),
        ]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );

        let event = logs.expect_one("nest_rs::openapi", "two operations share one operationId");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("operation_id").as_deref(), Some("posts_list"));
        let addresses = [event.field("path"), event.field("conflicting_path")];
        assert!(
            addresses.iter().flatten().any(|p| p == "/posts")
                && addresses.iter().flatten().any(|p| p == "/archive/posts"),
            "the warning names both operations: {event:#?}",
        );
        assert_eq!(event.field("method").as_deref(), Some("GET"));
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("rename one of the two controllers")),
            "and what to do about it: {event:#?}",
        );

        assert!(
            id_at(&doc, "/posts", "get").is_some()
                && id_at(&doc, "/archive/posts", "get").is_some()
        );
    }

    #[test]
    fn a_multi_version_controller_leaves_nothing_to_warn_about() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = deployment(vec![controller(
            "ReportsController",
            "reports",
            "/reports",
            &["1", "2"],
            vec![route("list", "/")],
        )]);
        build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert!(
            logs.find("nest_rs::openapi", "two operations share one operationId")
                .is_empty(),
            "the ids differ, so there is no collision to report: {:#?}",
            logs.events(),
        );
    }

    #[test]
    fn two_controllers_on_one_address_are_a_duplicate_mount_not_a_duplicate_id() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = deployment(vec![
            controller(
                "ThingsController",
                "things",
                "/things",
                &[],
                vec![route("list", "/")],
            ),
            controller(
                "ThingsController",
                "things",
                "/things",
                &[],
                vec![route("list", "/")],
            ),
        ]);
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
        assert_eq!(id_at(&doc, "/things", "get"), Some("things_list"));
        logs.expect_none("nest_rs::openapi", "two operations share one operationId");
    }
}
