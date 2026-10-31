//! Configuration failures.

use std::borrow::Cow;
use std::fmt::Write as _;

use nest_rs_core::DecodeError;
use thiserror::Error;
use validator::{ValidationErrors, ValidationErrorsKind};

/// A configuration load failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// A value refused, naming the offending variable.
    ///
    /// Built only through [`ConfigError::parse`], which strips any value a
    /// decoder quoted in the message.
    #[error("invalid value for {var}: {message}")]
    #[non_exhaustive]
    Parse {
        /// The offending `<PREFIX>_<DOMAIN>__<KEY>` variable name.
        var: String,
        /// Why the value was rejected.
        message: String,
    },
    /// A structured value that did not decode as its type.
    ///
    /// Carries [`DecodeError`], never serde's own sentence, which quotes the
    /// value it refused.
    #[error("invalid value for {var}{}: {source}", inside(.source))]
    Decode {
        /// The variable that supplied the value — `<KEY>` or `<KEY>_FILE`.
        var: String,
        /// Where and why it did not decode, without the value.
        #[source]
        source: DecodeError,
    },
    /// A duration's variable — its key ends in `_SECS` or `_MS` — asked of a
    /// [`ConfigService`](crate::ConfigService) reader, which holds a value to no
    /// range — a defect in the config's `from_env`.
    #[error(
        "{var} is a duration, and a duration is read through `DurationBounds`, which holds it \
         to a floor and a ceiling and names its unit — never through a `ConfigService` reader"
    )]
    UnboundedDuration {
        /// The variable the reader was asked for.
        var: String,
    },
    /// A vocabulary read under a field it does not name its variables by — a
    /// `ClientTls` (the `tls` feature's) under a field not named `tls`, whose
    /// checks would refuse `TLS_CA_CERT` for a variable spelled otherwise — a
    /// defect in the config's declaration.
    #[error(
        "{var}_* would hold a `{vocabulary}`, whose refusals name its variables {named}_*, the \
         field `{field}`'s, never the ones this field reads: name the field `{field}`"
    )]
    MisnamedField {
        /// The stem of the variables the field would read,
        /// `<PREFIX>_<DOMAIN>__<KEY>`.
        var: String,
        /// The vocabulary the field holds.
        vocabulary: &'static str,
        /// The one field name it is read under.
        field: &'static str,
        /// The stem its refusals name, the field's key under the namespace.
        named: String,
    },
    /// A duration field whose shape contradicts its bounds — an
    /// `Option<Duration>`, whose `None` is off, under bounds with no off, or a
    /// `Duration` under bounds whose `0` is off — a defect in the config's
    /// declaration.
    #[error(
        "`{field}` is {}, and the bounds {var} is read through {}: a duration is an \
         `Option<Duration>` where its bounds turn it off (`Floor::UnitsOrOff`), `None` off, \
         and a `Duration` everywhere else",
        if *.optional { "an `Option<Duration>`" } else { "a `Duration`" },
        if *.optional { "admit no off" } else { "turn it off at `0`" }
    )]
    MisshapenDuration {
        /// The variable the field is read from, `<PREFIX>_<DOMAIN>__<KEY>`.
        var: String,
        /// The field, as its bounds name it — `HttpConfig::request_timeout`.
        field: &'static str,
        /// Whether the field is an `Option<Duration>`.
        optional: bool,
    },
    /// A loaded config failed `validator::Validate`.
    ///
    /// Renders the namespace's variables and one line per offending field —
    /// the field, then the variable that sets it, since a value may come from
    /// a pinned base as well — never `validator`'s `Debug` payload, which
    /// holds the submitted value. A config its caller composed, read under no
    /// variable, is named by its type and its fields alone.
    #[error(
        "configuration validation failed for `{}`\n{}",
        validated(namespace),
        render(namespace, errors, whole_fields)
    )]
    #[non_exhaustive]
    Validation {
        /// The namespace the config was read under: its `#[config(namespace = "…")]`,
        /// with a named instance's name inserted after the vendor — or the
        /// config's type, when its caller composed it.
        namespace: Cow<'static, str>,
        /// The field-level failures. Not `#[source]`: a `Display` chain would
        /// print `validator`'s raw payload, submitted value included.
        errors: ValidationErrors,
        /// The fields read whole from their one variable — each
        /// `#[config(json)]` field — so a failure inside one names that
        /// variable.
        whole_fields: &'static [&'static str],
    },
    /// Two config types read one environment variable, raised at boot from the
    /// resolved name.
    #[error(
        "`{var}` is read by two configuration types — `{owner}` and `{claimant}`. \
         A `<PREFIX>_<DOMAIN>__<KEY>` variable belongs to one type: setting it would \
         configure whichever read it, with nothing to say which. Give one of the two \
         its own key — a field's name is its key — or its own namespace, read off \
         its own path."
    )]
    ContestedVariable {
        /// The fully-qualified variable both types read.
        var: String,
        /// The type that claimed it first.
        owner: &'static str,
        /// The type that claimed it second.
        claimant: &'static str,
    },
    /// Two configuration types declare one namespace, raised at the read of
    /// either type, whichever comes first.
    #[error(
        "the namespace `{namespace}` is declared by {} configuration types — {}. A namespace \
         belongs to one type, so that a variable under it names the type that reads it: give \
         each its own `#[config(namespace = \"…\")]`, read off its own path.",
        declarations.len(),
        declarations.iter().map(|d| format!("`{d}`")).collect::<Vec<_>>().join(" and "),
    )]
    SharedNamespace {
        /// The namespace both declare.
        namespace: &'static str,
        /// Every type declaring it, sorted.
        declarations: Vec<&'static str>,
    },
    /// A named instance whose namespace another declaration holds: a linked
    /// `#[config]` declaring it, or another marker reading the instance's name.
    #[error(
        "the instance `{instance}` reads the namespace `{namespace}`, which {} also claim{}: \
         an instance's namespace is its config's with the instance's name inserted after the \
         vendor, and a namespace belongs to one declaration, so that a variable under it names \
         what it configures. Rename the instance.",
        claims.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(" and "),
        if claims.len() == 1 { "s" } else { "" },
    )]
    SharedInstance {
        /// The marker of the instance being read, by its module path and ident.
        instance: &'static str,
        /// The namespace that instance reads.
        namespace: String,
        /// Every other declaration holding it, sorted: a `#[config]` type, or a
        /// marker reading the same name.
        claims: Vec<&'static str>,
    },
    /// A `<KEY>_FILE` variable names a file that could not be read.
    ///
    /// The variable's value is never carried: it may be key material pasted
    /// where a path belongs.
    #[error("could not read the file named by {var}")]
    File {
        /// The `<PREFIX>_<DOMAIN>__<KEY>_FILE` variable naming the file.
        var: String,
        /// Why reading it failed.
        #[source]
        source: std::io::Error,
    },
}

impl ConfigError {
    /// Build a [`Parse`](Self::Parse) error naming the variable and the reason.
    ///
    /// A parser's source excerpt (`1 | token = "…"`) is dropped from `message`,
    /// and a serde-quoted value removed, as [`DecodeError::redact`] does.
    pub fn parse(var: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Parse {
            var: var.into(),
            message: redacted(&message.into()),
        }
    }

    /// Build a [`Validation`](Self::Validation) error for `namespace`, whose
    /// fields are each read from a variable of their own.
    pub fn validation(namespace: impl Into<Cow<'static, str>>, errors: ValidationErrors) -> Self {
        Self::Validation {
            namespace: namespace.into(),
            errors,
            whole_fields: &[],
        }
    }

    /// Build a [`Parse`](Self::Parse) error for `key` of `namespace`, a value no
    /// tier set and no default fills: "must be set, inline or through
    /// `<KEY>_FILE`", then `why` there is no default to fall back on.
    ///
    /// ```
    /// use nest_rs_config::{ConfigError, var_name};
    ///
    /// let refused = ConfigError::unset("seaorm", "URL", "a deployment names its database");
    /// assert_eq!(
    ///     refused.to_string(),
    ///     format!(
    ///         "invalid value for {}: must be set, inline or through {} — a deployment names its \
    ///          database",
    ///         var_name("seaorm", "URL"),
    ///         var_name("seaorm", "URL_FILE"),
    ///     ),
    /// );
    /// ```
    pub fn unset(namespace: &str, key: &str, why: impl std::fmt::Display) -> Self {
        Self::parse(
            crate::var_name(namespace, key),
            format!(
                "must be set, inline or through {} — {why}",
                crate::var_name(namespace, &crate::service::file_key(key))
            ),
        )
    }
}

/// A config read from the container before any factory resolved it, as
/// [`ConfigModule::resolved`](crate::ConfigModule::resolved) refuses it: a
/// `register` files it with `ContainerBuilder::refuse`, a factory returns it.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "`{}`{of_instance} was read before any factory resolved it: the module owning it is not \
     imported, or is imported in a `register` alone, or the factory reading it does not name it \
     among its inputs",
    nest_rs_core::__private::shorten(.type_name),
    of_instance = of_instance(instance),
)]
pub struct ConfigNotResolvedError {
    /// The config type that was read.
    pub type_name: &'static str,
    /// The instance it was read for, as its namespace names it — empty for the
    /// default one ([`ConfigModule::resolved_for`](crate::ConfigModule::resolved_for)).
    pub instance: &'static str,
}

/// The words naming a named instance after its config's type; none for the
/// default one.
fn of_instance(instance: &str) -> String {
    if instance.is_empty() {
        String::new()
    } else {
        format!(" of the instance `{instance}`")
    }
}

/// Where inside a variable's value its decode failed, as the refusal says it
/// after the variable: `` at `clients[0].secret` ``, or nothing.
fn inside(report: &DecodeError) -> String {
    report
        .path()
        .map(|path| format!(" at `{path}`"))
        .unwrap_or_default()
}

/// `message` without the values a decoder quoted in it.
fn redacted(message: &str) -> String {
    let kept: Vec<&str> = message
        .lines()
        .filter(|line| !is_source_excerpt(line))
        .collect();
    DecodeError::redact(&kept.join("\n"), None).into_owned()
}

/// A line of a source excerpt — `1 | key = "…"`, or the gutter `  |  ^^^`
/// under it — which repeats the parsed text verbatim.
fn is_source_excerpt(line: &str) -> bool {
    line.trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start()
        .starts_with('|')
}

/// Whether a validation refusal's `namespace` is one — lowercase, as every
/// namespace is — rather than the type of a config its caller composed.
fn is_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// What a validation refusal is about: the namespace's variables, or a
/// composed config's type.
fn validated(namespace: &str) -> String {
    if is_namespace(namespace) {
        crate::var_name(namespace, "*")
    } else {
        namespace.to_owned()
    }
}

/// One `  - field (VARIABLE): rule` line per failure, deepest field path
/// flattened into a dotted name; the variable is the namespace's key for the
/// path — a member of a vocabulary extends its field's key with `_`, an item
/// of a list is the list's, and anything inside a field read whole is that
/// field's — and is left out for a composed config.
///
/// A rule's parameters are never said: `value` is the rejected input,
/// `must_match`'s `other` another field's, and a bound can read another field.
fn render(namespace: &str, errors: &ValidationErrors, whole_fields: &[&str]) -> String {
    let mut out = String::new();
    let fields = Fields {
        namespace,
        whole: whole_fields,
    };
    render_into(&mut out, &fields, errors, "");
    out.trim_end().to_owned()
}

/// What names a failing field's variable: the namespace it was read under,
/// and the fields read whole from one variable.
struct Fields<'a> {
    namespace: &'a str,
    whole: &'a [&'a str],
}

impl Fields<'_> {
    /// The variable that sets the field at `path`.
    fn variable(&self, path: &str) -> String {
        let field = path.split('[').next().unwrap_or(path);
        let top = field.split('.').next().unwrap_or(field);
        if self.whole.contains(&top) {
            return crate::var_name(self.namespace, top);
        }
        crate::var_name(self.namespace, &field.replace('.', "_"))
    }
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "fmt::Write for String never fails"
)]
fn render_into(out: &mut String, fields: &Fields<'_>, errors: &ValidationErrors, prefix: &str) {
    for (field, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        };
        match kind {
            ValidationErrorsKind::Field(list) => {
                let named = if is_namespace(fields.namespace) {
                    format!("{path} ({})", fields.variable(&path))
                } else {
                    path.clone()
                };
                for error in list {
                    let _ = writeln!(out, "  - {named}: {}", error.code);
                }
            }
            ValidationErrorsKind::Struct(nested) => render_into(out, fields, nested, &path),
            ValidationErrorsKind::List(items) => {
                for (index, nested) in items {
                    render_into(out, fields, nested, &format!("{path}[{index}]"));
                }
            }
        }
    }
}

/// A `Result` whose error is a [`ConfigError`].
pub type Result<T> = std::result::Result<T, ConfigError>;

#[cfg(test)]
mod tests {
    use validator::Validate;

    use super::*;

    #[derive(Validate)]
    struct Issuer {
        #[validate(length(min = 1))]
        client_id: String,
        #[validate(range(min = 1, max = 500))]
        page_size: u32,
    }

    fn rendered(namespace: &'static str, value: &impl Validate) -> String {
        let errors = value.validate().expect_err("must fail");
        ConfigError::validation(namespace, errors).to_string()
    }

    /// `  - field (VARIABLE): rule`, the variable under `namespace` for `key`.
    fn line(namespace: &str, field: &str, key: &str, rule: &str) -> String {
        format!(
            "\n  - {field} ({}): {rule}",
            crate::var_name(namespace, key)
        )
    }

    #[test]
    fn a_validation_failure_names_its_variables_and_lists_each_field_with_its_own() {
        let text = rendered(
            "issuer",
            &Issuer {
                client_id: String::new(),
                page_size: 900,
            },
        );

        assert!(
            text.starts_with(&format!(
                "configuration validation failed for `{}`",
                crate::var_name("issuer", "*")
            )),
            "the namespace's variables say which config failed: {text}",
        );
        assert!(
            text.contains(&line("issuer", "client_id", "CLIENT_ID", "length")),
            "{text}"
        );
        assert!(
            text.contains(&line("issuer", "page_size", "PAGE_SIZE", "range")),
            "{text}"
        );
    }

    #[derive(Validate)]
    struct Client {
        #[validate(length(min = 1))]
        secret: String,
    }

    #[derive(Validate)]
    struct Clients {
        #[validate(nested)]
        clients: Vec<Client>,
    }

    #[test]
    fn an_item_of_a_list_is_set_by_the_lists_variable() {
        let text = rendered(
            "issuer",
            &Clients {
                clients: vec![Client {
                    secret: String::new(),
                }],
            },
        );
        assert!(
            text.contains(&line("issuer", "clients[0].secret", "CLIENTS", "length")),
            "{text}"
        );
    }

    #[derive(Validate)]
    struct Limits {
        #[validate(range(max = 10))]
        max: u32,
    }

    #[derive(Validate)]
    struct Limited {
        #[validate(nested)]
        limits: Limits,
    }

    /// A field read whole — `#[config(json)]` — is set by its one variable,
    /// whatever inside it failed; a vocabulary's member keeps its own.
    #[test]
    fn a_failure_inside_a_field_read_whole_names_that_fields_variable() {
        let refused = |whole_fields: &'static [&'static str]| {
            let errors = Limited {
                limits: Limits { max: 99 },
            }
            .validate()
            .expect_err("must fail");
            ConfigError::Validation {
                namespace: "kiln".into(),
                errors,
                whole_fields,
            }
            .to_string()
        };
        let whole = refused(&["limits"]);
        assert!(
            whole.contains(&line("kiln", "limits.max", "LIMITS", "range")),
            "{whole}"
        );
        let vocabulary = refused(&[]);
        assert!(
            vocabulary.contains(&line("kiln", "limits.max", "LIMITS_MAX", "range")),
            "{vocabulary}"
        );
    }

    #[test]
    fn a_composed_config_is_named_by_its_type_and_its_fields_alone() {
        let text = rendered(
            "OAuthClientConfig",
            &Issuer {
                client_id: String::new(),
                page_size: 1,
            },
        );
        assert_eq!(
            text,
            "configuration validation failed for `OAuthClientConfig`\n  - client_id: length"
        );
    }

    const SECRET: &str = "sk_live_51HsecretTOKEN";

    #[derive(Validate)]
    struct Signing {
        signing_key: String,
        #[validate(must_match(other = "signing_key"))]
        signing_key_confirm: String,
        ceiling: u64,
        #[validate(range(max = self.ceiling))]
        burst: u64,
    }

    /// A rule's parameters can hold input: `must_match`'s `other` is another
    /// field's value, and a bound is an expression that can read one.
    #[test]
    fn a_validation_failure_says_its_rules_and_none_of_their_parameters() {
        let text = rendered(
            "signing",
            &Signing {
                signing_key: SECRET.into(),
                signing_key_confirm: "mistyped".into(),
                ceiling: 4_242_424_242,
                burst: 9_999_999_999,
            },
        );
        assert!(!text.contains("sk_live"), "{text}");
        assert!(!text.contains("4242424242"), "{text}");
        assert!(
            text.contains(&line(
                "signing",
                "signing_key_confirm",
                "SIGNING_KEY_CONFIRM",
                "must_match"
            )),
            "{text}"
        );
        assert!(
            text.contains(&line("signing", "burst", "BURST", "range")),
            "{text}"
        );
    }

    /// A JSON decoder's sentence handed to the sink whole, as a reader judging
    /// a value of its own would hand it, quotes nothing it refused.
    #[test]
    fn a_json_decode_failure_reaches_the_boot_error_without_its_value() {
        let error = serde_json::from_str::<u16>(&format!("\"{SECRET}\""))
            .expect_err("a string is not a port");
        assert!(
            error.to_string().contains(SECRET),
            "the probe quotes: {error}"
        );

        let var = crate::var_name("fixture", "PORT");
        let text = ConfigError::parse(var.clone(), error.to_string()).to_string();
        assert!(!text.contains(SECRET), "{text}");
        assert!(
            text.starts_with(&format!("invalid value for {var}: invalid type: ")),
            "the failure is still said: {text}"
        );
    }

    /// TOML quotes twice: the excerpt it opens with repeats the line it read,
    /// and serde's sentence after it quotes the value. Neither reaches the boot
    /// error — a type error or a syntax error alike.
    #[test]
    fn a_toml_decode_failure_reaches_the_boot_error_without_its_value() {
        use figment::providers::{Format, Toml};
        use std::collections::BTreeMap;

        for (document, what) in [
            (format!("port = \"{SECRET}\""), "invalid type"),
            (format!("token = {SECRET} trailing"), "TOML parse error"),
        ] {
            let error = Toml::from_str::<BTreeMap<String, u16>>(&document)
                .expect_err("the document does not decode");
            assert!(
                error.to_string().contains(SECRET),
                "the probe quotes: {error}"
            );

            let text =
                ConfigError::parse(crate::var_name("fixture", "SETTINGS"), error.to_string())
                    .to_string();
            assert!(!text.contains(SECRET), "{text}");
            assert!(text.contains(what), "the failure is still said: {text}");
        }
    }

    #[test]
    fn an_unset_value_names_both_spellings_and_why_there_is_no_default() {
        let text = ConfigError::unset("fixture", "TOKEN", "it has no default").to_string();
        assert_eq!(
            text,
            format!(
                "invalid value for {}: must be set, inline or through {} — it has no default",
                crate::var_name("fixture", "TOKEN"),
                crate::var_name("fixture", "TOKEN_FILE"),
            )
        );
    }

    /// A refusal that quotes nothing is said word for word.
    #[test]
    fn a_reason_with_nothing_quoted_is_kept_whole() {
        let text = ConfigError::parse("V", "must be at least 1 second — why").to_string();
        assert_eq!(text, "invalid value for V: must be at least 1 second — why");
    }

    #[test]
    fn the_submitted_value_and_the_bounds_are_never_said() {
        let text = rendered(
            "issuer",
            &Issuer {
                client_id: "ok".into(),
                page_size: 900,
            },
        );

        assert!(
            text.contains(&line("issuer", "page_size", "PAGE_SIZE", "range")),
            "{text}"
        );
        assert!(
            !text.contains("900"),
            "the submitted value must not be echoed: {text}",
        );
        assert!(!text.contains("500"), "no bound: {text}");
        assert!(!text.contains("Number("), "no raw debug payload: {text}");
    }
}
