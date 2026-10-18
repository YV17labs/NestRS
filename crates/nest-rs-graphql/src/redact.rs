//! Every error a GraphQL response carries, said without a value the request
//! sent: a decode failure's, a variable's, nor the `operationName`.
//!
//! async-graphql builds an error's `message` from the failure's `Display`, and
//! serde's sentence quotes the value; each message is said through
//! [`DecodeError::redact`]. Its validation and input coercion quote what a
//! variable held — an enum's value, an input object's unknown key, a scalar's
//! value — and a scalar's own refusal may quote it too; its refusal of an
//! `operationName` the document holds no operation of quotes that
//! `operationName`, the request's own text rather than a name the document
//! declares. Each is said by kind, as async-graphql 7.2 words it. A name the
//! document parsed under the GraphQL Name grammar — a field, an argument, a
//! fragment, a variable — stays, as the specification's own errors name it.
//!
//! An [`Executor`] wrapper rather than an extension, which would be entered on
//! every field of every request.

use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::{Data, Executor, Request, Response, ServerError};
use nest_rs_core::DecodeError;

/// The schema, answering with every error message redacted; a batch goes
/// through [`execute`](Executor::execute), the trait's default.
#[derive(Clone)]
pub(crate) struct Redacted<E>(pub(crate) E);

impl<E: Executor> Executor for Redacted<E> {
    async fn execute(&self, request: Request) -> Response {
        redacted(self.0.execute(request).await)
    }

    fn execute_stream(
        &self,
        request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, Response> {
        self.0
            .execute_stream(request, session_data)
            .map(redacted)
            .boxed()
    }
}

fn redacted(mut response: Response) -> Response {
    response.errors.iter_mut().for_each(redact);
    response
}

/// How async-graphql's refusal of an unknown `operationName` opens, the
/// `operationName` following it.
const UNKNOWN_OPERATION_QUOTED: &str = "Unknown operation named \"";

/// What that refusal says instead.
const UNKNOWN_OPERATION: &str = "Unknown operation named by operationName";

/// One error's message, redacted against its source when it is serde's or an
/// anyhow chain; any other source is read by serde's wording alone.
fn redact(error: &mut ServerError) {
    if error.source.is_none() {
        if error.path.is_empty() && error.message.starts_with(UNKNOWN_OPERATION_QUOTED) {
            UNKNOWN_OPERATION.clone_into(&mut error.message);
            return;
        }
        if let Some(said) = input_by_kind(&error.message) {
            error.message = said;
            return;
        }
    }
    let source = error.source.as_deref();
    let cause: Option<&(dyn std::error::Error + 'static)> = source
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .map(|error| error as &(dyn std::error::Error + 'static))
        .or_else(|| {
            source
                .and_then(|source| source.downcast_ref::<nest_rs_core::anyhow::Error>())
                .map(|error| error.as_ref() as &(dyn std::error::Error + 'static))
        });
    if let std::borrow::Cow::Owned(message) = DecodeError::redact(&error.message, cause) {
        error.message = message;
    }
}

/// The openings of async-graphql's validation of an argument's value, and of a
/// variable's default; a quoted path of declared names follows, then the reason.
const INVALID_VALUE: [&str; 2] = [
    "Invalid value for argument \"",
    "Invalid default value for argument: \"",
];

/// The reason that quotes an enum's value after the enum's name.
const ENUM_VALUE: &str = "enumeration type \"";
const ENUM_VALUE_QUOTED: &str = "\" does not contain the value \"";

/// The reason that quotes an input object's key before the object's name.
const UNKNOWN_FIELD: &str = "unknown field \"";
const UNKNOWN_FIELD_OF: &str = "\" of type \"";

/// `InputValueError::expected_type`: the type, then the value found.
const EXPECTED_TYPE: &str = "Expected input type \"";
const EXPECTED_TYPE_FOUND: &str = "\", found ";

/// `InputValueError::custom`: the type, then the scalar's own message.
const FAILED_TO_PARSE: &str = "Failed to parse \"";
const FAILED_TO_PARSE_BECAUSE: &str = "\": ";

/// `message` said by kind, when it is one of async-graphql's sentences quoting
/// what a variable, or the document, sent as a value.
fn input_by_kind(message: &str) -> Option<String> {
    if let Some(rest) = message.strip_prefix(EXPECTED_TYPE) {
        let (ty, found) = rest.split_once(EXPECTED_TYPE_FOUND)?;
        return Some(format!(
            "{EXPECTED_TYPE}{ty}\", found {}.",
            value_kind(found)
        ));
    }
    if let Some(rest) = message.strip_prefix(FAILED_TO_PARSE) {
        let (ty, because) = rest.split_once(FAILED_TO_PARSE_BECAUSE)?;
        return Some(format!(
            "{FAILED_TO_PARSE}{ty}\": {}",
            scalar_refusal(because)
        ));
    }
    let opening = INVALID_VALUE
        .iter()
        .find(|opening| message.starts_with(*opening))?;
    // The path names declared arguments, fields and indexes: no quote in it.
    let (path, reason) = message[opening.len()..].split_once("\", ")?;
    let said = if let Some(rest) = reason.strip_prefix(ENUM_VALUE) {
        let (name, _) = rest.split_once(ENUM_VALUE_QUOTED)?;
        format!("{ENUM_VALUE}{name}\" does not contain the value sent")
    } else {
        // The key is the sender's and may spell anything; the object's name,
        // declared, closes the sentence and holds no quote.
        let (_, name) = reason
            .strip_prefix(UNKNOWN_FIELD)?
            .rsplit_once(UNKNOWN_FIELD_OF)?;
        let name = name.strip_suffix('"')?;
        format!("an unknown field of type \"{name}\"")
    };
    Some(format!("{opening}{path}\", {said}"))
}

/// What async-graphql's rendering of a value is, read off how it opens: a
/// string is quoted, a list bracketed, an object braced, and an enum value is
/// a bare name, `true`, `false` and `null` excepted.
fn value_kind(rendered: &str) -> &'static str {
    let word = |name: &str| {
        rendered
            .strip_prefix(name)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
    };
    match rendered.as_bytes().first() {
        Some(b'"') => "a string",
        Some(b'[') => "a list",
        Some(b'{') => "an object",
        Some(b'-' | b'0'..=b'9') => "a number",
        _ if word("true") || word("false") => "a boolean",
        _ if word("null") => "null",
        Some(byte) if byte.is_ascii_alphabetic() || *byte == b'_' => "an enum value",
        _ => "a value",
    }
}

/// A scalar's refusal as [`DecodeError`] reports a type's own message: one of
/// serde's sentences rebuilt without the value, any other by its category.
fn scalar_refusal(message: &str) -> String {
    use nest_rs_core::__private::serde::de::Error as _;
    let refusal = nest_rs_core::__private::serde::de::value::Error::custom(message);
    DecodeError::from(&refusal).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of async-graphql 7.2's sentences quoting a value, as it renders
    /// one, and what it is said as.
    #[test]
    fn a_sentence_quoting_a_value_is_said_by_kind() {
        for (rendered, said) in [
            (
                r#"Invalid value for argument "role", enumeration type "Role" does not contain the value "sk_live""#,
                r#"Invalid value for argument "role", enumeration type "Role" does not contain the value sent"#,
            ),
            (
                r#"Invalid value for argument "filter.role", enumeration type "Role" does not contain the value "a" does not contain the value "b""#,
                r#"Invalid value for argument "filter.role", enumeration type "Role" does not contain the value sent"#,
            ),
            (
                r#"Invalid default value for argument: "role", enumeration type "Role" does not contain the value "sk_live""#,
                r#"Invalid default value for argument: "role", enumeration type "Role" does not contain the value sent"#,
            ),
            (
                r#"Invalid value for argument "filter", unknown field "sk_live" of type "Filter""#,
                r#"Invalid value for argument "filter", an unknown field of type "Filter""#,
            ),
            (
                r#"Invalid value for argument "filter.0", unknown field "a" of type "sk_live" of type "Filter""#,
                r#"Invalid value for argument "filter.0", an unknown field of type "Filter""#,
            ),
            (
                r#"Expected input type "Int", found "sk_live"."#,
                r#"Expected input type "Int", found a string."#,
            ),
            (
                r#"Expected input type "Int", found "sk_live". (occurred while parsing "Filter")"#,
                r#"Expected input type "Int", found a string."#,
            ),
            (
                r#"Expected input type "Code", found {card: "sk_live"}."#,
                r#"Expected input type "Code", found an object."#,
            ),
            (
                r#"Expected input type "Code", found [1, 2]."#,
                r#"Expected input type "Code", found a list."#,
            ),
            (
                r#"Expected input type "Code", found -4.2."#,
                r#"Expected input type "Code", found a number."#,
            ),
            (
                r#"Expected input type "Code", found true."#,
                r#"Expected input type "Code", found a boolean."#,
            ),
            (
                r#"Expected input type "Code", found trueish."#,
                r#"Expected input type "Code", found an enum value."#,
            ),
            (
                r#"Expected input type "Code", found null."#,
                r#"Expected input type "Code", found null."#,
            ),
            (
                r#"Failed to parse "Code": not a code: sk_live"#,
                r#"Failed to parse "Code": a value its type does not accept"#,
            ),
            (
                r#"Failed to parse "Role": Enumeration type does not contain value "sk_live"."#,
                r#"Failed to parse "Role": a value its type does not accept"#,
            ),
            (
                "Failed to parse \"JSON\": unknown variant `sk_live`, expected `Visa`",
                "Failed to parse \"JSON\": unknown variant, expected `Visa`",
            ),
            (
                "Failed to parse \"JSON\": invalid type: string \"sk_live\", expected u64",
                "Failed to parse \"JSON\": invalid type: a string, expected u64",
            ),
        ] {
            assert_eq!(input_by_kind(rendered).as_deref(), Some(said), "{rendered}");
        }
    }

    /// A sentence naming only declared types, arguments and fields, or the
    /// document's own names, is answered as written.
    #[test]
    fn a_sentence_quoting_no_value_is_left_as_written() {
        for rendered in [
            r#"Invalid value for argument "amount", expected type "Int""#,
            r#"Invalid value for argument "filter", field "name" of type "String!" is required but not provided"#,
            r#"Invalid value for argument "pick", Oneof input objects requires have exactly one field"#,
            r#"Unknown argument "limit" on field "posts" of type "Query"."#,
            r#"Variable "$id" is not defined by operation "Fetch""#,
            "the handle is taken",
        ] {
            assert_eq!(input_by_kind(rendered), None, "{rendered}");
        }
    }
}
