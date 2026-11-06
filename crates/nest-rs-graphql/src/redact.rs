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
//! `Context::data`'s refusal names a Rust type, and a resolver's `?` hands it on
//! as an answer of its own: it answers the opaque error, the type filed. The line
//! names no field: an error's path holds the aliases the client wrote, and the
//! field's own `graphql.field` line, under one trace, names it as it is served.
//!
//! An [`Executor`] wrapper rather than an extension, which would be entered on
//! every field of every request. A document that does not parse never reaches it
//! quoted: the request's extension refuses one through [`unparsed`].

use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::{Data, Executor, Pos, Request, Response, ServerError};
use nest_rs_core::DecodeError;

use crate::error::{UnheldData, opaque_error};

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

/// How `Context::data`'s refusal reads, the Rust type's path between the two.
const NO_DATA: (&str, &str) = ("Data `", "` does not exist.");

/// The Rust type `message` names, when it is `Context::data`'s refusal.
pub(crate) fn unheld_data(message: &str) -> Option<&str> {
    message.strip_prefix(NO_DATA.0)?.strip_suffix(NO_DATA.1)
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
        if let Some(data) = unheld_data(&error.message) {
            let opaque = opaque_error(&UnheldData {
                data: data.to_owned(),
            });
            error.message = opaque.message;
            error.extensions = opaque.extensions;
            error.source = opaque.source;
            return;
        }
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

/// How the parser opens the syntax error of a number it cannot hold, and all that
/// is said of it: the column its sentence goes on to name counts inside the number.
const INVALID_NUMBER: &str = "invalid number";

/// The parser's own sentences past its grammar, which quote nothing the document holds.
const PARSER_SAID: [&str; 3] = [
    "document contains multiple operations",
    "document does not contain an operation",
    "recursion limit exceeded.",
];

/// pest's own sentences closing a rendering, which name no rule.
const PEST_SAID: [&str; 3] = [
    "stack limit reached",
    "call limit reached",
    "unknown parsing error",
];

/// `refused`, the parser's failure of a document, by an allowlist: its own sentences
/// over what the document declares, else the place its `locations` name and the grammar's
/// words. A rendering quotes the source and pest floats, so what is not read is not said.
pub(crate) fn unparsed(mut refused: ServerError) -> ServerError {
    if parser_said(&refused.message) {
        return refused;
    }
    let at = match refused.locations.first() {
        Some(&Pos { line, column }) => format!("Syntax error at line {line} column {column}"),
        None => "Syntax error".to_owned(),
    };
    refused.message = match expected_by_grammar(&refused.message) {
        Some(expected) => format!("{at}: {expected}"),
        None => at,
    };
    refused
}

/// Whether `message` is one of [`PARSER_SAID`], or the parser's refusal of a name defined
/// twice, that name held to the Name grammar.
fn parser_said(message: &str) -> bool {
    PARSER_SAID.contains(&message)
        || ["operation ", "fragment "].iter().any(|opening| {
            message
                .strip_prefix(opening)
                .and_then(|rest| rest.strip_suffix(" is defined twice"))
                .is_some_and(is_name)
        })
}

/// What a syntax error's `message` says the grammar expected: a rendering's last line,
/// under the source it quotes, when pest's words make all of it — rule names after
/// `expected` or `unexpected`, or one of [`PEST_SAID`].
fn expected_by_grammar(message: &str) -> Option<&str> {
    if message.starts_with(INVALID_NUMBER) {
        return Some(INVALID_NUMBER);
    }
    let (_, last) = message.rsplit_once('\n')?;
    let said = last.trim_start().strip_prefix("= ")?;
    let rules = match said.strip_prefix("unexpected ") {
        Some(rest) => match rest.split_once("; expected ") {
            Some((found, expected)) => rule_list(found) && rule_list(expected),
            None => rule_list(rest),
        },
        None => said.strip_prefix("expected ").is_some_and(rule_list),
    };
    (rules || PEST_SAID.contains(&said)).then_some(said)
}

/// Whether `list` is rule names as pest enumerates them: `a`, `a or b`, `a, b, or c`.
fn rule_list(list: &str) -> bool {
    let Some((head, last)) = list.rsplit_once(" or ") else {
        return is_name(list);
    };
    let head = head.strip_suffix(',').unwrap_or(head);
    is_name(last) && head.split(", ").all(is_name)
}

/// Whether `text` is a GraphQL Name, which is how pest's rendering spells a rule too.
fn is_name(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    use async_graphql::parser::parse_query;

    use super::*;

    /// A document's parser failure, as the request's read hands it over.
    fn refused(query: &str) -> ServerError {
        unparsed(parse_query(query).expect_err("it does not parse").into())
    }

    /// Each shape the parser stops on holds a literal the reply must not: a string,
    /// a block string over lines, a line spelling the rendering's own last line, a
    /// line break the rendering cuts at, a multibyte character before the place.
    #[test]
    fn a_syntax_error_says_its_place_and_what_was_expected() {
        for (query, said) in [
            (
                r#"mutation { login(password: "zzqx" }"#,
                "Syntax error at line 1 column 35: expected name",
            ),
            (
                "zzqx",
                "Syntax error at line 1 column 1: expected executable_definition",
            ),
            (
                r#"{ charge(amount: "zzqx"#,
                "Syntax error at line 1 column 18: expected value",
            ),
            (
                "{ charge(amount: \"\"\"zzqx\nzzqx\nzzqx",
                "Syntax error at line 1 column 20: expected name",
            ),
            (
                "{\n= expected zzqx",
                "Syntax error at line 2 column 1: expected selection",
            ),
            (
                "{ charge(amount: 1) } zzqx",
                "Syntax error at line 1 column 23: expected EOI or executable_definition",
            ),
            (
                "{ charge(amount: 1)\r\n zzqx(\r\n",
                "Syntax error at line 3 column 1: expected name",
            ),
            (
                "{ é\"zzqx\" ( }",
                "Syntax error at line 1 column 3: expected selection",
            ),
            (
                "query zzqx",
                "Syntax error at line 1 column 11: expected variable_definitions, selection_set, \
                 or directive",
            ),
            (
                "",
                "Syntax error at line 1 column 1: expected executable_definition",
            ),
            (
                "{ charge(amount: 1e99999) }",
                "Syntax error at line 1 column 18: invalid number",
            ),
        ] {
            let refused = refused(query);
            assert_eq!(refused.message, said, "{query}");
            assert!(!refused.message.contains("zzqx"), "{query}");
        }
    }

    /// The parser's own bound on nesting stops at a place the stack left decides, on
    /// a line as long as the document.
    #[test]
    fn a_document_past_the_parsers_nesting_says_its_limit() {
        let refused = refused(&format!("{}zzqx", "{a".repeat(300_000)));
        assert!(
            refused
                .message
                .starts_with("Syntax error at line 1 column ")
        );
        assert!(
            refused.message.ends_with(": stack limit reached"),
            "{}",
            refused.message,
        );
    }

    /// The parser's place stays in `locations`, as the specification has it.
    #[test]
    fn a_syntax_error_keeps_its_location() {
        let refused = refused("{\n  charge(");
        assert_eq!(refused.locations.len(), 1);
        assert_eq!(
            (refused.locations[0].line, refused.locations[0].column),
            (2, 10)
        );
    }

    /// A rendering whose last line is not pest's words says the place alone, whatever
    /// it holds.
    #[test]
    fn a_syntax_error_of_any_other_shape_is_not_read() {
        for message in [
            "zzqx",
            "zzqx\nzzqx",
            "  |\n1 | zzqx\n  | ^---",
            "1:1: zzqx: expected name",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = zzqx",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = expected name, found \"zzqx\"",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = expected name, found zzqx",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = expected name: zzqx",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = unexpected zz qx",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = expected ",
            " --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = stack limit reached at zzqx",
        ] {
            assert_eq!(expected_by_grammar(message), None, "{message}");
        }
    }

    /// Each way pest's rendering closes.
    #[test]
    fn a_rendering_closing_on_pests_words_is_read() {
        for said in [
            "expected name",
            "expected EOI or executable_definition",
            "expected variable_definitions, selection_set, or directive",
            "unexpected name",
            "unexpected name or value; expected selection, field, or directive",
            "stack limit reached",
            "call limit reached",
            "unknown parsing error",
        ] {
            let rendering = format!(" --> 1:1\n  |\n1 | zzqx\n  | ^---\n  |\n  = {said}");
            assert_eq!(expected_by_grammar(&rendering), Some(said), "{rendering}");
        }
    }

    /// What it was handed decides, never a parse of its own: pest's nesting bound reads
    /// the stack left, so a second parse of the same document may well pass it.
    #[test]
    fn a_rendering_is_said_by_what_it_was_handed() {
        let rendering = |last: &str| {
            ServerError::new(
                format!(" --> 1:895\n  |\n1 | {{a{{a(b: \"zzqx\")}}}}\n  | ^---\n  |\n  = {last}"),
                Some(Pos {
                    line: 1,
                    column: 895,
                }),
            )
        };
        assert_eq!(
            unparsed(rendering("stack limit reached")).message,
            "Syntax error at line 1 column 895: stack limit reached",
        );
        assert_eq!(
            unparsed(rendering("zzqx")).message,
            "Syntax error at line 1 column 895",
        );
        assert_eq!(
            unparsed(ServerError::new("zzqx", None)).message,
            "Syntax error"
        );
    }

    /// The parser's other failures name what the document declares, and stay.
    #[test]
    fn a_parser_failure_that_is_no_syntax_error_is_left_as_written() {
        let nested_past_the_bound = format!("{}a{}", "{a".repeat(100), "}".repeat(100));
        for (query, said) in [
            ("{ a } { b }", "document contains multiple operations"),
            (
                "query A { a } query A { b }",
                "operation A is defined twice",
            ),
            (
                "{ a } fragment F on Q { a } fragment F on Q { b }",
                "fragment F is defined twice",
            ),
            (
                "fragment F on Q { a }",
                "document does not contain an operation",
            ),
            (nested_past_the_bound.as_str(), "recursion limit exceeded."),
        ] {
            assert_eq!(refused(query).message, said, "{query}");
        }
    }

    /// A sentence shaped like the parser's but holding what no Name holds is not one.
    #[test]
    fn a_sentence_naming_what_no_name_holds_says_the_place_alone() {
        for message in [
            "operation \"zzqx\" is defined twice",
            "fragment zz qx is defined twice",
            "fragment 1zzqx is defined twice",
            "operation  is defined twice",
            "subscription zzqx is defined twice",
        ] {
            assert_eq!(
                unparsed(ServerError::new(message, None)).message,
                "Syntax error",
                "{message}",
            );
        }
    }

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
