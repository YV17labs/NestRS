//! Assemble an OpenAPI 3.1 document from the discovered HTTP controllers.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::{Container, DiscoveryService};
use nest_rs_http::{
    ApiVersioning, GlobalGuardsActive, HttpConfig, HttpControllerMeta, HttpRouteMeta,
    MEDIA_TYPE_PARAM, declared_versions, join_path,
};
use poem::http::{StatusCode, header};
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::config::OpenApiConfig;

/// The `operationId` collisions already reported during one boot.
///
/// A collision is a property of the *route table*, so it is one defect however
/// many documents are built from it — and a deployment under a non-URI strategy
/// builds one document per declared version beside the default. Sharing this
/// across them is what keeps `warn` at *one event, said once* instead of three
/// identical lines with nothing to tell them apart.
#[derive(Default)]
pub struct Reported {
    ids: std::collections::HashSet<String>,
}

/// Build the OpenAPI document for everything mounted on the HTTP transport.
///
/// Called once at the transport's `configure` step (container fully assembled),
/// so it sees every controller. A single [`SchemaGenerator`] runs across all
/// routes so every `Json<T>` payload contributes to a shared
/// `components/schemas`.
///
/// `claims` names the API version this document describes, and only bites under
/// a non-URI versioning strategy: `None` — what `/api-json` passes — claims the
/// deployment's default version when it names one, and every declared version
/// otherwise; `Some(v)` is the per-version document `/api-json/v{v}`. Under the
/// URI strategy the version is part of every path, so one document already
/// names every address a client calls and `claims` is ignored.
///
/// `reported` is the boot-scoped ledger that keeps each diagnostic to one event
/// however many documents that boot builds.
pub fn build_document(
    container: &Container,
    config: &OpenApiConfig,
    claims: Option<&str>,
    reported: &mut Reported,
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
    let selection = VersionSelection::resolve(container, claims);

    let controllers = discovery.meta::<HttpControllerMeta>();
    // One entry per address a controller answers at: a `version = ["1", "2"]`
    // controller is two, and an unversioned one is still one. The document
    // describes addresses, so this is the unit it iterates.
    let mut entries: Vec<(&Arc<HttpControllerMeta>, Option<&'static str>)> = controllers
        .iter()
        .flat_map(|d| d.meta.mounted_versions().map(move |v| (&d.meta, v)))
        .collect();
    if selection.is_some() {
        // Sorted, so which operation a contested path keeps is decided by the
        // versions rather than by link order: `None` sorts below `Some`, and
        // the last write wins, so the highest version is the one described.
        //
        // "Highest" is a *natural* order, not a string one. `sort_by_key` over
        // `Option<&str>` compared lexicographically, which puts `"10"` below
        // `"9"` — so a v9/v10 pair described v9 while three places (this
        // comment, the docs page and the CHANGELOG) said otherwise.
        //
        // Only here. Ordering a JSON object changes nothing a client reads, but
        // it rewrites every line of a committed document — so under the URI
        // strategy, where no two controllers can contest a path, the order
        // stays the one discovery hands over.
        entries.sort_by(|(a_meta, a), (b_meta, b)| {
            compare_versions(*a, *b).then_with(|| a_meta.path.cmp(b_meta.path))
        });
    }

    let mut described: HashMap<(String, &'static str), Option<&'static str>> = HashMap::new();
    // Every `operationId` this document has published, and the operation that
    // published it — the ledger the uniqueness OpenAPI requires is checked
    // against.
    let mut operation_ids: HashMap<String, (String, &'static str)> = HashMap::new();
    let mut paths: Map<String, Value> = Map::new();
    for (meta, version) in &entries {
        let version = *version;
        if let Some(selection) = &selection
            && !selection.describes(version)
        {
            continue;
        }
        // Under a non-URI strategy the mounted prefix (`/v1/posts`) is not an
        // address any client may call — the transport 404s it — so the document
        // keys on the controller's own path and moves the version into a
        // parameter below.
        let prefix = match &selection {
            Some(_) => meta.path.to_owned(),
            None => meta.effective_prefix(version),
        };
        for route in &meta.routes {
            // A `#[version]`-narrowed route is not served under every version
            // its controller declares, and a document naming an address that
            // answers `404` is the defect this whole module exists to avoid.
            if !HttpControllerMeta::serves(route, version) {
                continue;
            }
            let full = join_path(&prefix, route.path);
            // A path OpenAPI has no template for. Reported rather than
            // published: the poem spelling reached `paths` verbatim, so a
            // generated client called `/blobs/*rest` as a literal URL. `warn`
            // rather than a boot failure, for the same reason a duplicate id is
            // — the route works, it is the *document* that cannot describe it.
            let Some(key) = openapi_path(&full) else {
                tracing::warn!(
                    target: crate::TARGET,
                    controller = meta.controller,
                    handler = route.handler,
                    path = %full,
                    "route omitted from the document: an OpenAPI path template is one whole \
                     segment, so a catch-all, an unnamed pattern, or a literal sharing a segment \
                     with a parameter cannot be described",
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
            // OpenAPI 3.1 §4.8.10.1 makes the id unique across the whole
            // document, so a generator can name a client method after it. The
            // controller and the version settle every collision the framework's
            // own shapes produce, so what reaches this branch is a naming clash
            // between two controllers whose names reduce to one token — rare,
            // and invisible everywhere else in the boot.
            //
            // Compared against the *address*, not merely counted: two
            // controllers mounted on one path overwrite each other's operation
            // here, so one id reaches the document and there is no collision to
            // report — that is a duplicate mount, and it is the transport's to
            // name.
            //
            // A warning, never a boot failure: what degrades is a generated
            // client, and an app is not stopped over its documentation.
            let address = (key.clone(), route.verb.as_str());
            if let Some(previous) = operation_ids.insert(id.clone(), address.clone())
                && previous != address
                // One defect, one event. The same clash reappears in every
                // document built from this route table, and three identical
                // lines with no field to tell them apart read as three problems.
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
            // OpenAPI keys one operation per (path, method), so two versions of
            // one client-facing path cannot both be described here. The claim is
            // recorded as it is made (the `insert` below runs for its return
            // value) and the loser is named rather than dropped in silence.
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
    // The RFC 9457 error body every failure renders (see `nest_rs_http::problem`).
    // Hand-written rather than derived so the doc has no build-time dependency on
    // the concrete struct's schemars derive.
    schemas.insert("ProblemDetails".into(), problem_details_schema());

    let mut info = json!({ "title": config.title, "version": config.version });
    if let (Some(description), Value::Object(info)) = (config.description.as_deref(), &mut info) {
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

/// The versions this deployment publishes a document of its own for —
/// `/api-json/v{n}` beside the default `/api-json`. Empty under the URI
/// strategy, where the version is part of every path and one document already
/// names every address a client calls.
pub(crate) fn versioned_documents(container: &Container) -> Vec<String> {
    match selects_per_request(container) {
        true => declared_versions(container),
        false => Vec::new(),
    }
}

/// Whether the deployment resolves the version **per request** (`header` /
/// `media_type`) rather than from the path.
pub(crate) fn selects_per_request(container: &Container) -> bool {
    container
        .get::<HttpConfig>()
        .is_some_and(|config| config.versioning != ApiVersioning::Uri)
}

/// How a document tells a caller to ask for an API version when the version is
/// not in the path (`NESTRS_HTTP__VERSIONING=header` / `media_type`).
///
/// Under those strategies the address a client calls is the unversioned one —
/// the URI form is a `404` — so the document keys on `#[controller(path = …)]`
/// and every operation that declares a version carries it as a header
/// parameter instead.
struct VersionSelection {
    /// [`ApiVersioning::Header`] or [`ApiVersioning::MediaType`]; the URI
    /// strategy produces no selection at all.
    strategy: ApiVersioning,
    /// The header a caller sets: the deployment's version header, or `Accept`
    /// for the media-type strategy.
    header: String,
    /// The deployment names no default version, so a caller that states none
    /// reaches no versioned route at all — stating one is not optional.
    required: bool,
    /// The one version this document describes, or `None` to describe every
    /// version the app declares.
    claims: Option<String>,
}

impl VersionSelection {
    /// `None` under the URI strategy: the version is part of the path there, so
    /// the document composed from the mounted route table is already the one a
    /// client calls.
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

    /// Whether this document describes the operations of `version`. An
    /// unversioned controller belongs to every document: it carries no version
    /// parameter, so nothing here tells a client to state one for it.
    fn describes(&self, version: Option<&str>) -> bool {
        match (version, &self.claims) {
            (Some(version), Some(claims)) => version == claims,
            _ => true,
        }
    }

    /// The version parameter one operation carries — the header a caller sets
    /// to reach `version`, `required` unless the deployment names a default.
    ///
    /// The `enum` holds what this document accepts for that operation: one
    /// value, because a document describes one version of a given path, and an
    /// enumerated string reaches a generated client as a typed choice rather
    /// than as free text.
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

/// A version as a log field — an unversioned controller has none, and the empty
/// string in a structured field says nothing to whoever reads it.
fn version_label(version: Option<&str>) -> &str {
    version.unwrap_or("unversioned")
}

/// The `operationId` an operation is published under: the controller that
/// serves it, its handler, and the version it is served under when it has one —
/// `posts_list`, `posts_list_v1`.
///
/// OpenAPI 3.1 §4.8.10.1 requires the id to be unique across the whole document
/// and a handler name alone never was: `#[crud]` names every resource's
/// operations `list`/`get`/`create`, so two resources in one document is already
/// a collision, and versioning adds two more shapes — one controller mounted
/// under two versions, and the two-controller layout. A generator meeting an id
/// twice either refuses the document or renames the loser, so the operations
/// reach a generated client as one method.
///
/// The controller settles what a document holds at one version; the version
/// settles what it holds across them. Qualifying by the controller is the
/// ecosystem's answer — `@nestjs/swagger` publishes `PostsController_findAll` —
/// spelled snake_case here because the id is what a generated client names a
/// method after, and `posts_list` reads as one.
///
/// Composed first and mapped once, by [`identifier_token`]: this is where the id
/// is assembled, so it is where the one rule about what an id may contain
/// belongs — no half of it arrives already safe.
fn operation_id(token: &str, handler: &str, version: Option<&str>) -> String {
    let qualified = format!("{token}_{handler}");
    let id = match version {
        Some(version) => format!("{qualified}_v{version}"),
        None => qualified,
    };
    identifier_token(&id)
}

/// An `operationId` as a generated client can carry it: every character an
/// identifier cannot hold becomes `_`.
///
/// **Run over the whole composed id, not over one half of it.** The characters
/// the composition itself contributes — the `_` joins and the `v` — are ones the
/// map keeps, so one pass over the join is the same string as three passes over
/// the parts, and a reader has one rule to hold rather than a per-half table.
/// Half a sanitised id is worse than none: the version half was mapped and the
/// other two were not, so `#[version("2024-08-11")]` published
/// `list_v2024_08_11` while `async fn r#type` published `probe_r#type`.
///
/// **No half is safe by construction, the controller token included.** A version
/// is an opaque string (`"2"`, but also `"2024-08-11"`). A handler is the
/// method's ident *as written*, so a raw one (`r#type`, `r#move`) arrives
/// carrying its `r#`. And the token is `snake_case` over the controller's ident,
/// which lowercases and inserts `_` but neither adds nor removes anything else —
/// so `struct r#Type` reaches here as `r#_type`. Rust's own identifier grammar
/// is what leaves the gap: `#` is legal in a name and illegal in an
/// `operationId`.
///
/// The map is deliberately lossy — `1.2` and `1-2` land on one token, as do
/// `r#type` and `r_type` — because the alternative is an id a client generator
/// has to mangle on its own terms. Two operations that collide here share one
/// id, which the document's own uniqueness check reports by name.
fn identifier_token(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// What to do about two operations claiming one `operationId`, which says what
/// is actually left to report: an id is `<controller>_<handler>` mapped onto
/// what an identifier can carry, so the two operations either declared one
/// handler name under controller names that reduce to one token, or two names
/// the map itself collapsed. Both remedies are a rename, and the sentence names
/// the two places one can happen because the ledger cannot tell which it caught.
/// A constant rather than a built sentence — unlike the contested-path remedy,
/// nothing here is named by a deployment's environment.
const DUPLICATE_ID_REMEDY: &str = "rename one of the two controllers, or one of the two \
     handlers: an operationId is <controller>_<handler> mapped onto what an identifier can \
     carry, so names that differ only by the `Controller` suffix, by casing (`Posts`, \
     `PostsController`) or by a character that map replaces (`r#type`, `r_type`) publish one \
     id — which OpenAPI requires to be unique across the document";

/// What to do about two versions on one documented path. Built rather than
/// spelled: the deployment's own prefix renames the variable.
fn contested_path_remedy() -> String {
    format!(
        "read the per-version document at /api-json/v{{n}}, or name the version /api-json \
         describes with {}",
        nest_rs_config::var_name("http", "DEFAULT_VERSION"),
    )
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

/// Compose one operation object. `operation_id` is composed by the caller, as
/// `full_path` is: both are what the *document* calls this operation, which the
/// route alone does not decide — see [`operation_id`].
fn operation_object(
    route: &HttpRouteMeta,
    full_path: &str,
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

    let mut parameters = typed_path_parameters(full_path, route.path_params, generator);
    parameters.extend(expand_object_params(route.query_params, "query", generator));
    parameters.extend(expand_object_params(
        route.header_params,
        "header",
        generator,
    ));
    parameters.extend(version_parameter);
    // A parameter the caller must send is a `400` the operation can actually
    // produce, on the same reading that ties `400` to a request body. One rule
    // over every emitted parameter, because a required query property and a
    // required header are the same rejection: `400` problem+json, naming the
    // field. Read off the emitted parameters rather than off the schema fns,
    // because "required" is what the expansion just decided.
    //
    // A path parameter is exempt and is the only one: it is part of the URL, so
    // omitting it does not reach this operation at all — it reaches another
    // route, or none.
    let requires_parameter = parameters
        .iter()
        .any(|p| p["required"] == true && p["in"] != "path");
    if !parameters.is_empty() {
        op.insert("parameters".into(), Value::Array(parameters));
    }

    if let Some(body) = route.request_body {
        let schema = match body.schema() {
            Some(schema_fn) => schema_fn(generator).to_value(),
            // A bare `Multipart` parameter: the media type is known, the parts
            // are not. A free-form object says exactly that — and says it to a
            // generated client, which silence never did.
            None => json!({ "type": "object", "additionalProperties": true }),
        };
        op.insert(
            "requestBody".into(),
            json!({ "required": true, "content": media_content(body.media_type(), schema) }),
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
    // A `204 No Content` and a `3xx` redirect carry no response body.
    let has_body = status != 204 && !is_redirect(status);
    let schema = has_body.then_some(route.response).flatten();
    // An ability-shaped route publishes its full shape and says so: the caller
    // receives whichever of these properties its ability grants. Saying nothing
    // — the previous behaviour — typed every `#[crud]` response as `any` in a
    // generated client (OAPI-O5).
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
    // What the success body arrives as: `application/json` unless the route
    // declared otherwise (`#[api(response_content_type = …)]`) or returns an
    // `SSE` stream. A declared media type with no schema is still documented —
    // a streamed download and an event stream both have a body, and typing it
    // `string` (`format: binary` off the text types) is what OpenAPI spells for
    // a body a schema cannot describe.
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
    // The success pendant of the `429`'s `Retry-After` below: a `#[crud]`
    // create's `201` names the row it minted and a `#[redirect]` names its
    // target, both in `Location`. Declared for the same reason — a generated
    // client only reads headers the document lists.
    if route.sets_location {
        ok.insert("headers".into(), location_header(status));
    }
    responses.insert(status.to_string(), Value::Object(ok));
    for (status, title) in error_statuses(route, full_path, global_guards, requires_parameter) {
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
    let names = path_parameter_names(path);
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
/// schema, filed under `location` (`query` or `header`) — this is how the
/// `#[crud]` list op's `Query<PageParams>` surfaces `first` and `after`, and how
/// a `Header<T>` surfaces the headers it reads. A property absent from the
/// schema's `required` is an optional parameter.
///
/// One expansion for both because they are the same shape: a struct whose
/// properties are named, flat, scalar-ish values. Only the `in` differs, and a
/// second copy would drift the day one side learned about `$ref`s.
fn expand_object_params(
    params: &[nest_rs_http::SchemaFn],
    location: &str,
    generator: &mut SchemaGenerator,
) -> Vec<Value> {
    let mut out = Vec::new();
    for schema_fn in params {
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
                    "in": location,
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
/// path binds an id, `409` only on a `#[crud]` write, `400` only with a body or
/// a parameter the caller must send.
fn error_statuses(
    route: &HttpRouteMeta,
    full_path: &str,
    global_guards: bool,
    requires_parameter: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if route.request_body.is_some() || requires_parameter {
        // The framework's edge validation (`Valid`/`Piped`) rejects with a `400`
        // RFC-9457 problem+json (see `nest_rs_http::pipe::reject`), not `422` —
        // the generated document must state the status clients will actually see
        // (OAPI-O2). `Query<T>` and `Header<T>` reject a missing required
        // property the same way, and a version token that does not parse is
        // refused before it reaches a path.
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
    if !path_parameter_names(full_path).is_empty() {
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

/// A `3xx`. Written once because two decisions must agree by construction: a
/// redirect carries no response body, and its `Location` points at a target
/// rather than at a row it created.
fn is_redirect(status: u16) -> bool {
    (300..400).contains(&status)
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

/// The media type a body is documented under when nothing declares another.
const JSON_MEDIA_TYPE: &str = "application/json";

/// A `content` map with one entry: `{"<media type>": {"schema": …}}`. Built
/// rather than written inline because the key is a value here, not a literal.
fn media_content(media_type: &str, schema: Value) -> Value {
    let mut content = Map::new();
    content.insert(media_type.to_owned(), json!({ "schema": schema }));
    Value::Object(content)
}

/// The body schema for a response the framework knows the media type of and
/// nothing more — a streamed download, an event stream. OpenAPI spells an
/// opaque byte stream `string` / `format: binary`; a `text/*` stream is text,
/// so it carries no binary format and claims none.
///
/// The test is case-insensitive because RFC 9110 §8.3.1 says the type is:
/// `TEXT/CSV` names the same media type as `text/csv`, and a bytewise
/// `starts_with` typed it as opaque bytes.
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

/// The `Location` response header a route that mints or points elsewhere
/// carries. An **absolute-path reference** on both paths the framework emits
/// (`/orgs/<id>`, a redirect target), hence `uri-reference` rather than `uri`.
///
/// `required` is deliberately absent, as it is on `Retry-After`: a `#[crud]`
/// create omits the header for an entity that does not key on a `Uuid`, and the
/// document would be claiming a guarantee the route does not make.
fn location_header(status: u16) -> Value {
    let description = if is_redirect(status) {
        "URI to follow for this resource."
    } else {
        "URI of the resource that was just created."
    };
    json!({
        "Location": {
            "description": description,
            "schema": { "type": "string", "format": "uri-reference" }
        }
    })
}

/// Order two versions the way a reader does: unversioned below versioned, then
/// **naturally** — digit runs compared as numbers, so `2` < `9` < `10` and
/// `2024-08-11` < `2024-09-01`.
///
/// A version is a free-form token (`#[controller(version = "2024-08-11")]` is
/// declarable), so this cannot parse it as one number. Comparing digit runs
/// numerically and everything else bytewise is what makes both shapes order the
/// way they read, and it is the only property "the highest version wins" needs.
fn compare_versions(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => natural_cmp(a, b),
    }
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    /// The leading run of digits with its leading zeros dropped, and what
    /// follows it. Leading zeros are spelling, not significance, so `007` and
    /// `7` compare equal and `007` sorts below `10`.
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
                // More significant digits is a larger number; same count falls
                // back to the digits themselves.
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

/// What one poem path segment is, in the terms OpenAPI can express.
///
/// The single reading of a path segment in this module. It was three: the path
/// rewriter, the parameter derivation and the `404` test each did their own
/// `strip_prefix(':')`, and all three therefore agreed only about `:name` —
/// which is why the other three forms poem's own matcher parses
/// (`nest_rs_http`'s `segment_matches`) reached the document verbatim, with no
/// parameter and no `404`.
enum Segment<'a> {
    /// A fixed segment, published as written.
    Literal(&'a str),
    /// A whole-segment parameter, named. `:id` and `:id<\d+>` alike — the
    /// pattern constrains the value, not the shape of the template.
    Parameter(&'a str),
    /// A segment OpenAPI has no template for, and this is a property of the
    /// standard rather than of this code. A path template is a whole segment
    /// (RFC 6570 level 1 as OpenAPI profiles it), so poem's catch-all (`*rest`),
    /// its unnamed regex segment (`<\d+>`) and a literal sharing a segment with
    /// a parameter (`/@:handle`) have no expression here.
    Untemplatable,
}

fn classify_segment(seg: &str) -> Segment<'_> {
    let Some(rest) = seg.strip_prefix(':') else {
        // A pattern or a catch-all anywhere else in the segment, or a literal
        // sharing the segment with one, has no whole-segment template.
        return match seg.contains([':', '<', '*']) {
            true => Segment::Untemplatable,
            false => Segment::Literal(seg),
        };
    };
    // `:name`, or `:name<pattern>` — the name runs to the pattern, which
    // constrains the value rather than the shape of the template.
    let name = rest.split_once('<').map_or(rest, |(name, _pattern)| name);
    match name.is_empty() {
        true => Segment::Untemplatable,
        false => Segment::Parameter(name),
    }
}

/// The parameters a path declares, in order.
fn path_parameter_names(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|seg| match classify_segment(seg) {
            Segment::Parameter(name) => Some(name),
            _ => None,
        })
        .collect()
}

/// poem path syntax (`/users/:id`) → OpenAPI syntax (`/users/{id}`), or `None`
/// when a segment has no OpenAPI template.
///
/// `None` is a refusal, not a fallback. Publishing the poem spelling put
/// `"/blobs/*rest"` in `paths` as a literal address, with no parameter and no
/// `404` — a generated client would call that URL verbatim. An operation the
/// standard cannot describe is left out and reported, which is the difference
/// between an incomplete document and a wrong one.
fn openapi_path(path: &str) -> Option<String> {
    let mut out = Vec::new();
    for seg in path.split('/') {
        match classify_segment(seg) {
            Segment::Literal(seg) => out.push(seg.to_owned()),
            Segment::Parameter(name) => out.push(format!("{{{name}}}")),
            Segment::Untemplatable => return None,
        }
    }
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use nest_rs_http::{DEFAULT_VERSION_HEADER, HttpVerb, RequestBodyMeta};
    use schemars::JsonSchema;
    use schemars::generate::SchemaSettings;
    use serde::Serialize;

    use super::*;

    #[test]
    fn joins_and_converts_paths() {
        assert_eq!(join_path("/users", "/:id"), "/users/:id");
        assert_eq!(openapi_path("/users/:id").as_deref(), Some("/users/{id}"));
        assert_eq!(join_path("/", "/"), "/");
    }

    #[test]
    fn openapi_path_handles_root_and_no_params() {
        assert_eq!(openapi_path("/").as_deref(), Some("/"));
        assert_eq!(openapi_path("/users").as_deref(), Some("/users"));
        assert_eq!(openapi_path("").as_deref(), Some(""));
    }

    #[test]
    fn openapi_path_handles_multiple_params() {
        assert_eq!(
            openapi_path("/orgs/:org_id/users/:id").as_deref(),
            Some("/orgs/{org_id}/users/{id}"),
        );
    }

    /// Every segment form poem's own matcher parses, and what the standard can
    /// say about each. The three that cannot be templated used to reach `paths`
    /// verbatim — a generated client called `/blobs/*rest` as a literal URL.
    #[test]
    fn openapi_path_refuses_what_the_standard_cannot_template() {
        // A pattern constrains the value, not the shape of the template.
        assert_eq!(
            openapi_path("/users/:id<\\d+>").as_deref(),
            Some("/users/{id}"),
        );

        for path in [
            "/blobs/*rest",    // a catch-all spans segments
            "/blobs/<\\d+>",   // an unnamed pattern names no parameter
            "/users/@:handle", // a literal sharing a segment with a parameter
            "/users/:",        // a parameter with no name
        ] {
            assert!(
                openapi_path(path).is_none(),
                "{path} has no OpenAPI path template",
            );
        }
    }

    /// "The highest version is the one described" was a string comparison, so a
    /// v9/v10 pair described v9 — deterministic, and the opposite of what this
    /// module's own comment, the docs page and the CHANGELOG all state.
    #[test]
    fn versions_order_the_way_they_read() {
        use std::cmp::Ordering;

        assert_eq!(compare_versions(Some("9"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("2"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("10"), Some("10")), Ordering::Equal);
        // Unversioned sorts below every version, so a versioned controller wins
        // a contested path.
        assert_eq!(compare_versions(None, Some("1")), Ordering::Less);
        assert_eq!(compare_versions(None, None), Ordering::Equal);
        // A date version is not a number, and still orders as it reads.
        assert_eq!(
            compare_versions(Some("2024-08-11"), Some("2024-09-01")),
            Ordering::Less,
        );
        assert_eq!(
            compare_versions(Some("2024-09-01"), Some("2025-01-01")),
            Ordering::Less,
        );
        // Leading zeros are spelling, not significance.
        assert_eq!(compare_versions(Some("007"), Some("10")), Ordering::Less);
        assert_eq!(compare_versions(Some("01"), Some("1")), Ordering::Equal);
        // A sort over the whole set lands where a reader expects.
        let mut all = [Some("10"), None, Some("9"), Some("1"), Some("2")];
        all.sort_by(|a, b| compare_versions(*a, *b));
        assert_eq!(all, [None, Some("1"), Some("2"), Some("9"), Some("10")]);
    }

    /// The `404` test and the parameter derivation read the path through the
    /// same classifier the rewriter does — they each had their own
    /// `strip_prefix(':')`, which is why all three agreed only about `:name`.
    #[test]
    fn path_parameters_are_read_one_way() {
        assert_eq!(
            path_parameter_names("/orgs/:org_id/users/:id"),
            ["org_id", "id"]
        );
        assert_eq!(path_parameter_names("/users/:id<\\d+>"), ["id"]);
        assert!(path_parameter_names("/blobs/*rest").is_empty());
        assert!(path_parameter_names("/users/@:handle").is_empty());
        assert!(path_parameter_names("/static/css").is_empty());
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

    /// The optional-property twin — what an `Option<_>` field on a header DTO
    /// produces, and the input for "an optional parameter advertises no `400`".
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
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            success_status: 200,
            scoped_guarded: false,
            public: false,
            versions: &[],
        }
    }

    /// The controller token the operation-body tests are served under — they
    /// are about what an operation *says*, not what it is called.
    const HOST_TOKEN: &str = "test";

    /// One operation as an unversioned deployment publishes it. The id is
    /// derived here rather than spelled, so a test about the body of an
    /// operation cannot pin an id the document would never have given it.
    fn operation(
        route: &HttpRouteMeta,
        full_path: &str,
        generator: &mut SchemaGenerator,
        global_guards: bool,
        version_parameter: Option<Value>,
    ) -> Value {
        operation_object(
            route,
            full_path,
            &operation_id(HOST_TOKEN, route.handler, None),
            generator,
            global_guards,
            version_parameter,
        )
    }

    #[test]
    fn a_path_param_segment_advertises_404_but_a_literal_colon_does_not() {
        // OAPI-O4: `404` on a route that binds an id segment (`:id`) — Path OR
        // Bind — but not on a static segment that merely contains a colon.
        let bound = error_statuses(&route("get_user", "/users/:id"), "/users/:id", false, false);
        assert!(
            bound.iter().any(|(s, _)| *s == "404"),
            "an `:id` route advertises 404",
        );

        let literal = error_statuses(&route("weird", "/a:b/list"), "/a:b/list", false, false);
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
        let op = operation(&r, "/audio/uploads", &mut g, false, None);
        let too_many = &op["responses"]["429"];
        assert_eq!(too_many["description"], "Too Many Requests");
        assert!(
            too_many["headers"]["Retry-After"]["schema"]["type"] == "integer",
            "429 must document the Retry-After header: {too_many}",
        );
    }

    /// R10: the `Location` a `#[crud]` create ships went undeclared while the
    /// `429` two lines away declared its `Retry-After` for the same stated
    /// reason. A generated client reads the document, not the prose, so an
    /// undeclared header is one no client will ever look at.
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

    /// The other producer: a redirect's response *is* its `Location`, and its
    /// description points at a target rather than a new row.
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

    /// The negative half — the document states what the framework knows it
    /// emitted. A plain `200` route declares no header it does not send.
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
        let statuses = error_statuses(&route("list", "/audio"), "/audio", false, false);
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
        let r = route("get_user", "/users/:id");
        let op = operation(&r, "/users/:id", &mut g, false, None);
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

    /// A `#[api(multipart = T)]` upload files its schema under the media type
    /// it actually arrives as — the gap that left `/audio/uploads/direct` with
    /// no `requestBody` at all.
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

    /// A handler pulling the parts itself types none of them — but the media
    /// type it accepts is still knowledge a client needs, so it is stated with
    /// a free-form object rather than omitted.
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

    /// A streamed download: the framework knows what it sends, not what shape
    /// the bytes have. OpenAPI's spelling for that is `string`/`format: binary`.
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

    /// An event stream is text, so it claims no binary format — the half of the
    /// rule that stops `text/event-stream` being described as bytes.
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
        // `response = T` states the shape, `response_content_type` states how it
        // travels — a JSON-shaped body served as `application/problem+json`, an
        // NDJSON feed. Neither overrides the other.
        let mut g = generator();
        let mut r = route("export", "/exports");
        r.response = Some(schema_for_dummy);
        r.response_content_type = Some("application/x-ndjson");
        let op = operation(&r, "/exports", &mut g, false, None);
        assert!(op["responses"]["200"]["content"]["application/x-ndjson"]["schema"].is_object());
    }

    #[test]
    fn a_bodyless_success_declares_no_streamed_content() {
        // A `204` carries no body, whatever media type the route declared.
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
        // The honesty half: a header nobody has to send cannot produce the
        // missing-header rejection.
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
        // No `response` fn → no content block on 200.
        assert!(op["responses"]["200"].get("content").is_none());
    }

    #[test]
    fn operation_object_attaches_response_schema_when_present() {
        let mut g = generator();
        let mut r = route("get_user", "/users/:id");
        r.response = Some(schema_for_dummy);
        let op = operation(&r, "/users/:id", &mut g, false, None);
        assert!(op["responses"]["200"]["content"]["application/json"]["schema"].is_object());
    }

    // OAPI-O5: a shaper masks *fields*, so the schema is published and the
    // description says the field set is ability-dependent. Publishing nothing
    // — the previous behaviour — typed every `#[crud]` response as `any` in a
    // generated client.
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

    // A `204` masked route has no body to describe, so the caveat would be
    // noise — the description stays the plain reason phrase.
    #[test]
    fn a_masked_bodyless_response_keeps_the_plain_description() {
        let mut g = generator();
        let mut r = route("delete_user", "/users/:id");
        r.response = Some(schema_for_dummy);
        r.masked = true;
        r.success_status = 204;
        let op = operation(&r, "/users/:id", &mut g, false, None);
        assert_eq!(op["responses"]["204"]["description"], "No Content");
        assert!(op["responses"]["204"].get("content").is_none());
    }

    #[test]
    fn a_non_200_success_status_replaces_the_200_response() {
        // OAPI-O3: `#[http_code(201)]` advertises `201 Created`, not `200`, and
        // still carries the body schema.
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
        // A `204 No Content` (a `#[crud]` delete) and a `3xx` redirect advertise
        // no response body even when a return schema exists (OAPI-O3).
        for (status, reason) in [(204, "No Content"), (307, "Temporary Redirect")] {
            let mut g = generator();
            let mut r = route("delete_user", "/users/:id");
            r.success_status = status;
            r.response = Some(schema_for_dummy);
            let op = operation(&r, "/users/:id", &mut g, false, None);
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
        let op = operation(&r, "/users", &mut g, true, None);
        assert_eq!(op["security"][0]["bearerAuth"], json!([]));
        assert!(op["responses"].get("401").is_some());
        assert!(op["responses"].get("403").is_some());
    }

    #[test]
    fn a_public_route_stays_unsecured_even_under_a_global_guard_pool() {
        let mut g = generator();
        let mut r = route("health", "/health");
        r.public = true;
        let op = operation(&r, "/health", &mut g, true, None);
        assert!(op.get("security").is_none());
        assert!(op["responses"].get("401").is_none());
    }

    /// The `info` block a document under test is built from. Named apart from
    /// the API *version* a document claims — the two are different strings and
    /// they meet in `build_document`'s arguments.
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
    fn no_servers_field_without_a_global_prefix() {
        let container = Container::builder().build();
        let doc = build_document(
            &container,
            &info("X", "0", None),
            None,
            &mut Reported::default(),
        );
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

    /// The versioning view a deployment publishes, from the one config that
    /// decides it. `versions` is what the controllers declare — the container
    /// holds no `HttpControllerMeta` here, so the selection is exercised
    /// directly rather than through a booted app (the integration suite does
    /// that half).
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
        // The regression risk of the whole feature: under `uri` the mounted
        // path *is* the client-facing one, so nothing about the document moves.
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
        // The version rides in a media-type parameter, so the value a client
        // sends is a media range — not a bare version, which is what an enum of
        // versions would have told it to send.
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

        // A route that answers in another media type is asked for in that one.
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

        // `/api-json` claims the default version without being told which.
        let default = selection(ApiVersioning::Header, Some("1"), None).expect("a selection");
        assert!(default.describes(Some("1")));
        assert!(!default.describes(Some("2")));

        // With no default named, one document describes every version.
        let all = selection(ApiVersioning::Header, None, None).expect("a selection");
        assert!(all.describes(Some("1")) && all.describes(Some("2")));
    }

    #[test]
    fn a_versioned_operation_advertises_the_400_its_version_token_can_produce() {
        // The version parameter is a parameter like any other: a caller that
        // must send one can send a malformed one, and that is a `400` before
        // the request reaches a path.
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
        // The half that used to disagree with headers: `expand_object_params`
        // marked the property required and the error set said nothing.
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
        // Path parameters are required by construction; omitting one reaches
        // another route or none, so it is a `404`, never this operation's `400`.
        let mut g = generator();
        let op = operation(
            &route("get", "/users/:id"),
            "/users/:id",
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

    /// One controller as discovery hands it over. `token` is what `#[routes]`
    /// derives from `name` and the document reads for every `operationId`; it
    /// is stated here rather than re-derived, so these tests assert the
    /// contract instead of reimplementing the macro's half of it.
    fn controller(
        name: &'static str,
        token: &'static str,
        path: &'static str,
        versions: &'static [&'static str],
        routes: Vec<HttpRouteMeta>,
    ) -> HttpControllerMeta {
        HttpControllerMeta::new(name, token, path, versions, routes, |_, route| route)
    }

    /// The five routes `#[crud]` writes for a resource, which is where the
    /// collisions live: every resource in the app declares these same names.
    fn crud_routes() -> Vec<HttpRouteMeta> {
        let mut list = route("list", "/");
        let mut get = route("get", "/:id");
        let mut create = route("create", "/");
        create.verb = HttpVerb::Post;
        let mut update = route("update", "/:id");
        update.verb = HttpVerb::Patch;
        let mut delete = route("delete", "/:id");
        delete.verb = HttpVerb::Delete;
        list.tags = &["crud"];
        get.tags = &["crud"];
        vec![list, get, create, update, delete]
    }

    /// A deployment of nothing but controllers — no `HttpConfig`, so the version
    /// is where the URI strategy puts it: in the path.
    fn deployment(controllers: Vec<HttpControllerMeta>) -> Container {
        controllers
            .into_iter()
            .fold(Container::builder(), |builder, meta| {
                builder.provide_meta(meta)
            })
            .build()
    }

    /// The `operationId` at one address, or `None` when the document names no
    /// operation there.
    fn id_at<'a>(document: &'a Value, path: &str, method: &str) -> Option<&'a str> {
        document["paths"][path][method]["operationId"].as_str()
    }

    /// Every `operationId` a document publishes, in no particular order.
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
        // The regression this rule exists for, and the one every nestrs app
        // meets: `#[crud]` names each resource's operations `list`/`get`/…, so
        // two resources in one document published ten operations under five ids
        // — five collisions, in an app that never asked for anything unusual.
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
        // The other collision: `version = ["1", "2"]` mounts `list` at two
        // addresses, so the controller alone does not tell them apart.
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
        // Under `header`/`media-type` the mounted prefix is not an address, so
        // both versions of `/posts` key to the same OpenAPI path — and OpenAPI
        // has one operation per (path, method). One of the two is dropped, and
        // this line is the only place that fact exists: the served document
        // simply describes v2, indistinguishable from a deployment that only
        // ever had v2.
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
        // Which of the two survived is the whole content of the event: named
        // the wrong way round it sends a reader to check the version that is
        // in fact published.
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
        // The other direction, and what keeps the warning meaningful: header
        // versioning alone is not a collision. Without this, a deployment that
        // versions every controller would warn on all of them.
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
        // The token itself — `PostsController` → `posts`, and the `Controller`
        // suffix that says nothing about which one this is — is `#[routes]`'
        // half, asserted in `nest-rs-http-macros`. What this crate owns is how
        // the two halves join.
        assert_eq!(operation_id("posts", "list", None), "posts_list");
        assert_eq!(
            operation_id("audio_uploads", "get", None),
            "audio_uploads_get"
        );
    }

    #[test]
    fn a_raw_ident_handler_reaches_the_document_as_an_identifier() {
        // `async fn r#type` is legal Rust and mounts like any other handler, and
        // what `#[routes]` records is the ident *as written* — so the `r#` used
        // to travel into the id, where `#` is not something a client generator
        // can name a method after.
        let id = operation_id("probe", "r#type", None);
        assert_eq!(id, "probe_r_type");
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "an operationId a generator can name a method after: {id}",
        );
    }

    #[test]
    fn a_raw_ident_controller_is_mapped_by_the_same_rule() {
        // The half the audit left to decide: `snake_case` lowercases and inserts
        // `_`, it does not drop anything, so `struct r#Type` arrives here as
        // `r#_type`. One map over the composed id answers for all three halves.
        assert_eq!(operation_id("r#_type", "list", None), "r__type_list");
        assert_eq!(
            operation_id("r#_type", "r#type", Some("2024-08-11")),
            "r__type_r_type_v2024_08_11",
        );
    }

    #[test]
    fn two_handler_spellings_that_map_to_one_id_are_reported_by_the_ledger() {
        // What sanitising can now create: `r#type` and `r_type` are two handlers
        // to Rust and one id to the document. No second mechanism for it — this
        // is the collision the uniqueness ledger already exists to name.
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

        // Degraded documentation, not a broken app — both are still published.
        assert!(
            id_at(&doc, "/probe/raw", "get").is_some()
                && id_at(&doc, "/probe/plain", "get").is_some()
        );
    }

    #[test]
    fn a_version_that_is_not_a_bare_integer_still_reads_as_an_identifier() {
        // A version is opaque — a date is as legal as `2` — but the id it lands
        // in becomes a method name in a generated client, so what an identifier
        // cannot carry is mapped, not passed through.
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
        // What is left once the controller and the version have done their work:
        // two names that differ only by the suffix every controller carries. The
        // document cannot publish both, and nothing else in the boot says so.
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

        // Degraded documentation, not a broken app: both operations are still
        // published, and the boot that emitted this went on.
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
        // The id is claimed once because the operation is written once — the
        // second controller overwrote the first. Reporting a shared id here
        // would name a document that does not exist; the duplicate *mount* is
        // the transport's to name.
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
