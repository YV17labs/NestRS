//! Covers `src/pipe.rs`: what every built-in `Pipe` that refuses client input
//! says when it does. An edge renders a refusal as it is — an HTTP `400`, a WS
//! frame and its line, a GraphQL or MCP error, a queue's dead-letter record and
//! line — so a refusal names what was expected, never what was sent.

use std::str::FromStr;

use nest_rs_pipes::{
    Parse, ParseArray, ParseBool, ParseFloat, ParseInt, ParseUuid, ParseUuidV3, ParseUuidV4,
    ParseUuidV5, ParseUuidV7, Pipe, ValidationPipe,
};
use validator::Validate;

use crate::{SECRET, carried, quoted_run};

/// A token minted as a v4 UUID: it parses, so it reaches the version check.
const UUID_SECRET: &str = "0b9a8c3e-5d2f-4a1b-9c7e-6f1d2e3a4b5c";

/// The same token under an NCS variant, refused before its version is read.
const NCS_SECRET: &str = "0b9a8c3e-5d2f-4a1b-1c7e-6f1d2e3a4b5c";

/// An enum whose own parser quotes what it refuses, so a pipe forwarding the
/// parser's words would forward the input.
enum Plan {
    Free,
    Pro,
}

impl FromStr for Plan {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "free" => Ok(Self::Free),
            "pro" => Ok(Self::Pro),
            other => Err(format!("no plan is called `{other}`")),
        }
    }
}

/// Every rule broken by a field carrying the secret, one of them a level down.
#[derive(Validate)]
struct Signup {
    #[validate(length(max = 8))]
    token: String,
    #[validate(email)]
    email: String,
    #[validate(nested)]
    referral: Referral,
}

#[derive(Validate)]
struct Referral {
    #[validate(length(max = 8))]
    code: String,
}

/// A confirmation that does not match the password it confirms, the password
/// being the secret.
#[derive(Validate)]
struct PasswordChange {
    password: String,
    #[validate(must_match(other = "password"))]
    confirmation: String,
}

/// Every built-in pipe that can refuse client input names what it expected and
/// never what it was sent, in its message or its details, whichever branch
/// refuses it. `Trim`, `Lowercase` and `Uppercase` never refuse.
#[test]
fn no_built_in_pipe_quotes_what_it_refuses() {
    let signup = Signup {
        token: SECRET.to_owned(),
        email: SECRET.to_owned(),
        referral: Referral {
            code: SECRET.to_owned(),
        },
    };
    let change = PasswordChange {
        password: SECRET.to_owned(),
        confirmation: "a different password".to_owned(),
    };
    let refusals = [
        ("ParseInt", SECRET, ParseInt::transform(SECRET.into()).err()),
        (
            "ParseBool",
            SECRET,
            ParseBool::transform(SECRET.into()).err(),
        ),
        (
            "ParseFloat",
            SECRET,
            ParseFloat::transform(SECRET.into()).err(),
        ),
        (
            "Parse<Plan>",
            SECRET,
            Parse::<Plan>::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuid",
            SECRET,
            ParseUuid::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuidV3",
            SECRET,
            ParseUuidV3::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuidV4",
            SECRET,
            ParseUuidV4::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuidV5",
            SECRET,
            ParseUuidV5::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuidV7",
            SECRET,
            ParseUuidV7::transform(SECRET.into()).err(),
        ),
        (
            "ParseUuidV7, sent a v4",
            UUID_SECRET,
            ParseUuidV7::transform(UUID_SECRET.into()).err(),
        ),
        (
            "ParseUuidV4, sent an NCS UUID",
            NCS_SECRET,
            ParseUuidV4::transform(NCS_SECRET.into()).err(),
        ),
        (
            "ParseArray<u64>",
            SECRET,
            ParseArray::<u64>::transform(format!("1,{SECRET},3")).err(),
        ),
        (
            "ParseArray<Plan>",
            SECRET,
            ParseArray::<Plan>::transform(format!("free,{SECRET}")).err(),
        ),
        (
            "ValidationPipe<Signup>",
            SECRET,
            ValidationPipe::<Signup>::transform(signup).err(),
        ),
        (
            "ValidationPipe<PasswordChange>",
            SECRET,
            ValidationPipe::<PasswordChange>::transform(change).err(),
        ),
    ];

    let mut quoting = Vec::new();
    for (pipe, secret, refusal) in refusals {
        let refusal = refusal.unwrap_or_else(|| panic!("{pipe} refuses what it was sent"));
        let said = carried(&refusal);
        if let Some(run) = quoted_run(&said, secret) {
            quoting.push(format!("{pipe} quotes `{run}`: {said}"));
        }
    }
    assert!(
        quoting.is_empty(),
        "refusals quoting what they were sent: {quoting:#?}"
    );
}
