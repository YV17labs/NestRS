//! An operation's typed arguments — decoded by the framework, and refused
//! without the value that did not decode.
//!
//! rmcp's own `Parameters<P>` answers a decode failure with serde's sentence
//! (`failed to deserialize parameters: invalid type: string "…", expected u64`),
//! which quotes what the client sent. That sentence goes back to a language
//! model, whose transcript keeps it and may repeat it to whoever is chatting —
//! the same reader [`Opaque`](crate::Opaque) exists for. So the framework's
//! `Parameters<P>` is its own: rmcp's shape, rmcp's schema, and a refusal worded
//! by [`DecodeError`].

use nest_rs_core::DecodeError;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::common::FromContextPart;
use rmcp::handler::server::prompt::PromptContext;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::JsonObject;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

/// What a refusal opens with — rmcp's own words, and they are load-bearing:
/// rmcp's tool router answers an `invalid_params` that opens with them as a
/// tool result flagged `isError` rather than a JSON-RPC error, which is how the
/// MCP specification asks an input error to reach the model (so it can correct
/// the call). Pinned by the suite, so an rmcp release rewording it fails here.
const REFUSED: &str = "failed to deserialize parameters";

/// A tool's or a prompt's typed arguments, deserialized from the call.
///
/// Used exactly as rmcp's: one `Parameters<T>` argument per operation,
/// destructured as `Parameters(args): Parameters<T>`, with `T`'s JSON Schema
/// advertised as the operation's input — rmcp's `#[tool]` / `#[prompt]` find the
/// argument by its name. A call whose arguments do not decode is refused with
/// `invalid_params` naming where and what kind, never the value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Parameters<P>(pub P);

impl<P: JsonSchema> JsonSchema for Parameters<P> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        P::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        P::json_schema(generator)
    }
}

impl<P: DeserializeOwned> Parameters<P> {
    /// `arguments` decoded as `P`; none at all decode as an empty object, so a
    /// type whose every field is optional binds.
    fn decode(arguments: Option<JsonObject>) -> Result<Self, McpError> {
        let arguments = serde_json::Value::Object(arguments.unwrap_or_default());
        serde_json::from_value(arguments)
            .map(Self)
            .map_err(|error| {
                McpError::invalid_params(format!("{REFUSED}: {}", DecodeError::new(&error)), None)
            })
    }
}

impl<S, P: DeserializeOwned> FromContextPart<ToolCallContext<'_, S>> for Parameters<P> {
    fn from_context_part(context: &mut ToolCallContext<'_, S>) -> Result<Self, McpError> {
        Self::decode(context.arguments.take())
    }
}

impl<S, P: DeserializeOwned> FromContextPart<PromptContext<'_, S>> for Parameters<P> {
    fn from_context_part(context: &mut PromptContext<'_, S>) -> Result<Self, McpError> {
        Self::decode(context.arguments.take())
    }
}
