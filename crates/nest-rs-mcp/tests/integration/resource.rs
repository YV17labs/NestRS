//! `src/resource.rs`: a `#[resource]` read and a `#[resources]` listing,
//! declared under `#[tools]` beside a tool and run through a site of their own,
//! a template variable held to the rules its type declares before the body.

use std::sync::Mutex;

use nest_rs_core::{Layer, async_trait, injectable, input, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_mcp::model::Resource;
use nest_rs_mcp::rmcp::serde_json::{Value, json};
use nest_rs_mcp::{Json, McpError, McpOperationKind, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, initialize, open_session, result};

const PATH: &str = "/mcp/notes";

/// What the host's guard was told of each operation it checked.
#[injectable]
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<String>>,
}

impl Layer for Recorder {}

#[async_trait]
impl Guard for Recorder {
    async fn check(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let view = cx
            .switch_to::<nest_rs_mcp::McpView>()
            .ok_or_else(|| Denial::internal("an MCP operation has the MCP view"))?;
        let kind = view
            .kind()
            .ok_or_else(|| Denial::internal("a declared operation names its role"))?;
        assert_eq!(
            cx.handler().address(),
            view.name().unwrap_or_default(),
            "the site the unit runs as is the operation's own",
        );
        assert!(
            view.host()
                .is_some_and(|host| host.ends_with("::NotesHost")),
            "the view names its host type",
        );
        self.seen.lock().expect("touched here alone").push(format!(
            "{kind} {} {}",
            view.method(),
            view.name().unwrap_or_default(),
        ));
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for Recorder {}

/// Refuses every operation it is bound to.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("not for you"))
    }
}

impl GuardFor<nest_rs_mcp::McpView> for Closed {}

/// Around every request, outside any operation's site: what the request's own
/// view says of each resource method.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct RequestViews {
    seen: Mutex<Vec<String>>,
}

impl Layer for RequestViews {
    fn serves(&self, unit: nest_rs_core::operation_log::Unit) -> bool {
        unit == nest_rs_mcp::unit::OPERATION
    }
}

#[async_trait]
impl nest_rs_core::Interceptor for RequestViews {
    async fn intercept(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
        next: nest_rs_core::Next<'_>,
    ) -> nest_rs_core::Outcome {
        if let Some(view) = cx.switch_to::<nest_rs_mcp::McpView>()
            && view.method().starts_with("resources/")
        {
            self.seen.lock().expect("touched here alone").push(format!(
                "{} {} {}",
                view.method(),
                view.kind().map_or("-", McpOperationKind::as_str),
                view.name().unwrap_or("-"),
            ));
        }
        next.run(cx).await
    }
}

#[input]
struct Note {
    id: u32,
    text: String,
}

#[mcp(path = "/mcp/notes")]
#[use_guards(Recorder)]
#[derive(Clone, Default)]
struct NotesHost;

#[tools]
impl NotesHost {
    #[resources]
    #[public]
    async fn notes(&self) -> Result<Vec<Resource>, McpError> {
        Ok(vec![Resource::new("note://1", "first")])
    }

    /// One note, as the JSON a client parses.
    #[resource(template = "note://{id}", title = "A note")]
    #[public]
    async fn note(&self, id: u32) -> Result<Option<Json<Note>>, McpError> {
        Ok((id == 1).then(|| {
            Json(Note {
                id,
                text: "the first note".to_owned(),
            })
        }))
    }

    #[resource(
        template = "note://{id}/text",
        name = "note_text",
        description = "A note's text alone."
    )]
    #[public]
    fn text(&self, id: u32) -> Result<String, McpError> {
        Ok(format!("the text of note {id}"))
    }

    /// A secret no caller reads.
    #[resource(template = "secret://{name}")]
    #[public]
    #[use_guards(Closed)]
    async fn secret(&self, name: String) -> Result<String, McpError> {
        Ok(format!("never reached: {name}"))
    }

    /// A tool beside the resources keeps its own role.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(
    imports = [crate::open_mcp()],
    providers = [NotesHost, Recorder, Closed, RequestViews]
)]
struct NotesModule;

async fn boot() -> TestApp {
    TestApp::for_module::<NotesModule>()
        .await
        .expect("a host declaring resources beside a tool boots")
}

/// `method` with `params` on a fresh session, its JSON-RPC answer decoded.
async fn ask(app: &TestApp, method: &str, params: Value) -> (String, Value) {
    let session = open_session(app.http(), PATH, None).await;
    let body = call_method(app.http(), PATH, &session, None, method, params).await;
    let decoded = result(&body);
    (body, decoded)
}

async fn read(app: &TestApp, uri: &str) -> (String, Value) {
    ask(app, "resources/read", json!({ "uri": uri })).await
}

#[tokio::test]
async fn the_resources_capability_is_derived_from_the_roles_present() {
    let app = boot().await;
    let body = initialize(app.http(), PATH, None).await;
    let advertised = &result(&body)["result"]["capabilities"];

    assert!(
        advertised["resources"].is_object(),
        "a `#[resource]` advertises the resources capability: {advertised}",
    );
    assert!(
        advertised["tools"].is_object(),
        "…beside the tool the same block declares: {advertised}",
    );
}

#[tokio::test]
async fn every_template_is_advertised_with_what_it_declares() {
    let app = boot().await;
    let (_, answer) = ask(&app, "resources/templates/list", json!({})).await;
    let templates = answer["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("a template listing: {answer}"));

    assert_eq!(templates.len(), 3, "{templates:?}");
    assert_eq!(
        templates[0],
        json!({
            "uriTemplate": "note://{id}",
            "name": "note",
            "title": "A note",
            "description": "One note, as the JSON a client parses.",
            "mimeType": "application/json",
        }),
        "the method names the template and the doc comment describes it",
    );
    assert_eq!(templates[1]["name"], "note_text");
    assert_eq!(templates[1]["description"], "A note's text alone.");
    assert_eq!(templates[1]["mimeType"], "text/plain");
    assert_eq!(templates[2]["uriTemplate"], "secret://{name}");
}

#[tokio::test]
async fn the_listing_answers_resources_list() {
    let app = boot().await;
    let (_, answer) = ask(&app, "resources/list", json!({})).await;

    assert_eq!(
        answer["result"]["resources"],
        json!([{ "uri": "note://1", "name": "first" }]),
    );
}

#[tokio::test]
async fn a_read_parses_the_template_s_variables_into_the_method_s_arguments() {
    let app = boot().await;

    let (_, answer) = read(&app, "note://1").await;
    assert_eq!(
        answer["result"]["contents"],
        json!([{
            "uri": "note://1",
            "mimeType": "application/json",
            "text": r#"{"id":1,"text":"the first note"}"#,
        }]),
    );

    let (_, answer) = read(&app, "note://7/text").await;
    assert_eq!(
        answer["result"]["contents"],
        json!([{
            "uri": "note://7/text",
            "mimeType": "text/plain",
            "text": "the text of note 7",
        }]),
        "a value never holds a reserved character, so `7/text` is the second template's",
    );
}

/// The same answer for a row the body did not find, a variable that does not
/// parse as its argument and a URI no template writes, none quoting what was
/// asked: a reader learns nothing it may not read.
#[tokio::test]
async fn a_resource_no_template_reads_is_not_found_quoting_nothing() {
    let app = boot().await;

    for asked in ["note://2", "note://two", "nowhere://1"] {
        let (body, answer) = read(&app, asked).await;
        assert_eq!(
            answer["error"],
            json!({
                "code": -32002,
                "message": "Resource not found",
                "data": { "code": "NOT_FOUND" },
            }),
            "{asked}: {body}",
        );
        assert!(!body.contains(asked), "{asked}: {body}");
    }
}

#[tokio::test]
async fn a_resource_runs_the_guards_its_site_binds() {
    let app = boot().await;

    let (body, answer) = read(&app, "secret://plans").await;
    assert_eq!(answer["error"]["message"], "not for you", "{body}");
    assert!(
        !body.contains("never reached"),
        "the body never ran: {body}"
    );
}

#[tokio::test]
async fn the_host_s_guard_is_told_each_resource_operation() {
    let app = boot().await;
    read(&app, "note://1").await;
    ask(&app, "resources/list", json!({})).await;
    ask(&app, "resources/templates/list", json!({})).await;

    let recorder = app
        .container()
        .get::<Recorder>()
        .expect("the guard is a provider");
    assert_eq!(
        *recorder.seen.lock().expect("touched in the guard alone"),
        [
            "resource resources/read note://{id}",
            "resources resources/list notes",
        ],
        "a read and a listing are operations the host's guard checks, named by the template \
         and the method — never by the values the URI carries — and the template listing, \
         which runs no method, is none",
    );
}

/// A unit-stage interceptor reads the request before any template matched it:
/// a read is a resource operation from there, named by the URI the request
/// sent, which only the host's site turns into its template.
#[tokio::test]
async fn a_resource_read_is_a_resource_operation_from_the_request_s_own_view() {
    let app = boot().await;
    read(&app, "note://1").await;
    ask(&app, "resources/list", json!({})).await;

    let views = app
        .container()
        .get::<RequestViews>()
        .expect("the interceptor is a provider");
    assert_eq!(
        *views.seen.lock().expect("touched in the interceptor alone"),
        ["resources/read resource note://1", "resources/list - -"],
        "a read names its role and the URI it asks for; a listing names nothing until the \
         host's site runs it",
    );
}

#[test]
fn a_resource_operation_kind_names_the_method_reaching_it() {
    assert_eq!(McpOperationKind::Resource.as_str(), "resource");
    assert_eq!(McpOperationKind::Resource.method(), "resources/read");
    assert_eq!(McpOperationKind::Resources.as_str(), "resources");
    assert_eq!(McpOperationKind::Resources.method(), "resources/list");
}

/// Listed by an operation's `#[use_guards]`, provided by no module.
#[injectable]
#[derive(Default)]
struct NeverProvided;

impl Layer for NeverProvided {}

#[async_trait]
impl Guard for NeverProvided {
    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for NeverProvided {}

#[mcp(path = "/mcp/orphan-resource")]
#[derive(Clone, Default)]
struct OrphanResourceHost;

#[tools]
impl OrphanResourceHost {
    /// A resource whose guard nothing provides.
    #[resource(template = "orphan://{id}")]
    #[public]
    #[use_guards(NeverProvided)]
    async fn orphan(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

#[module(imports = [crate::open_mcp()], providers = [OrphanResourceHost])]
struct OrphanResourceModule;

#[tokio::test]
async fn a_resource_whose_guard_no_module_provides_refuses_the_boot() {
    let refused = match TestApp::for_module::<OrphanResourceModule>().await {
        Ok(_) => panic!("a resource's site composes at boot like a tool's"),
        Err(err) => format!("{err:#}"),
    };
    assert!(refused.contains("NeverProvided"), "{refused}");
}

/// A template variable whose type declares rules.
#[input]
struct Slug {
    #[validate(length(min = 3))]
    value: String,
}

impl std::str::FromStr for Slug {
    type Err = std::convert::Infallible;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            value: value.to_owned(),
        })
    }
}

const TAGS: &str = "/mcp/tags";

#[mcp(path = "/mcp/tags")]
#[derive(Clone, Default)]
struct TagsHost;

#[tools]
impl TagsHost {
    /// One tag, read off its slug.
    #[resource(template = "tag://{slug}")]
    #[public]
    async fn tag(&self, slug: Slug) -> Result<String, McpError> {
        Ok(format!("the tag {}", slug.value))
    }
}

#[module(imports = [crate::open_mcp()], providers = [TagsHost])]
struct TagsModule;

#[tokio::test]
async fn a_template_variable_breaking_its_rules_is_refused_before_the_body() {
    let app = TestApp::for_module::<TagsModule>()
        .await
        .expect("a host reading a variable whose type declares rules boots");
    let read = |uri: &'static str| {
        let app = &app;
        async move {
            let session = open_session(app.http(), TAGS, None).await;
            let body = call_method(
                app.http(),
                TAGS,
                &session,
                None,
                "resources/read",
                json!({ "uri": uri }),
            )
            .await;
            let answer = result(&body);
            (body, answer)
        }
    };

    let (body, refused) = read("tag://zq").await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{body}");
    assert_eq!(refused["error"]["message"], "validation failed", "{body}");
    assert_eq!(
        refused["error"]["data"]["code"],
        nest_rs_core::problem::code::INVALID_ARGUMENT.as_str(),
        "{body}",
    );
    assert_eq!(
        refused["error"]["data"]["errors"]["value"][0]["code"], "length",
        "{body}"
    );
    assert!(!body.contains("the tag"), "the body ran: {body}");
    assert!(!body.contains("zq"), "the refusal quotes the value: {body}");

    let (body, kept) = read("tag://abc").await;
    assert_eq!(
        kept["result"]["contents"][0]["text"], "the tag abc",
        "{body}"
    );
}
