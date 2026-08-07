use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use features::users::UsersMcpModule;
use nest_rs::authn::ProtectedResourceModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::mcp::{McpEndpoint, McpModule};
use nest_rs::redis::QueueModule;
use nest_rs::seaorm::DatabaseModule;
use nest_rs::server_timing::ServerTimingModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        // The app names the endpoint its features share — a client sees one
        // server, so the name, the version and the instructions framing the
        // whole surface are the *app's* to write, not `audio`'s or `users`'.
        McpModule::endpoint(
            McpEndpoint::new("/mcp", "nestrs-assistant", env!("CARGO_PKG_VERSION"))
                .title("nestrs demo assistant")
                .instructions(
                    "Tools over the demo's own data. `transcribe_audio` queues a \
                     transcription and returns its job id; `list_people` reads the \
                     directory. Every call is scoped to the caller's token — an \
                     empty result means not authorized, not absent.",
                ),
        )
        // A second endpoint is a second server, so it is named too — and a lone
        // host is exactly where forgetting costs the most: undeclared, rmcp's
        // default makes the endpoint introduce itself to every client as the
        // SDK, `rmcp 3.x`.
        .endpoint(
            McpEndpoint::new("/posts/mcp", "nestrs-assistant-posts", env!("CARGO_PKG_VERSION"))
                .title("nestrs demo assistant — posts"),
        ),
        ProtectedResourceModule::for_root(None),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        // `audio` and `users` both serve `/mcp`; `posts` keeps its own
        // `/posts/mcp`. One endpoint aggregating two features is what lets each
        // keep its own `mcp/` adapter — and a second endpoint staying distinct
        // is what keeps the spec's per-endpoint tool namespacing usable.
        AudioMcpModule,
        UsersMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;
