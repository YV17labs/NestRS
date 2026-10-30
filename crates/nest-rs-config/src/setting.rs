//! [`Setting`] — a value as the deployment gave it, and the spelling that
//! supplied it.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use nest_rs_core::DecodeError;
use serde::de::DeserializeOwned;

use crate::error::ConfigError;

/// One key's value and the variable that supplied it: `<KEY>` when it was
/// inline, `<KEY>_FILE` when it was read from the file that variable names.
///
/// A consumer judging the value refuses it through [`refuse`](Self::refuse),
/// naming the variable and what it expected, and never quoting the value,
/// inline or read from a file: a deployment's configuration is somebody's data,
/// and a refusal is logged.
///
/// `Debug` shows the variable and never the value.
#[derive(Clone)]
pub struct Setting<T = String> {
    /// The value: text for [`ConfigService::setting`](crate::ConfigService::setting),
    /// a [`Material`](crate::Material) for
    /// [`ConfigService::material`](crate::ConfigService::material).
    pub value: T,
    var: String,
    from_file: bool,
}

impl<T> Setting<T> {
    pub(crate) fn new(value: T, var: String, from_file: bool) -> Self {
        Self {
            value,
            var,
            from_file,
        }
    }

    /// The fully-qualified variable that supplied the value — `<KEY>` or
    /// `<KEY>_FILE`.
    pub fn var(&self) -> &str {
        &self.var
    }

    /// Whether the value was read from the file `<KEY>_FILE` names.
    pub fn from_file(&self) -> bool {
        self.from_file
    }

    /// The boot error refusing this value for `reason`, naming the variable that
    /// supplied it. `reason` says what was expected and must not quote the value.
    pub fn refuse(&self, reason: impl fmt::Display) -> ConfigError {
        ConfigError::parse(self.var.clone(), reason.to_string())
    }
}

impl Setting {
    /// The value parsed as `T`, refused under this setting's variable when it
    /// does not parse.
    ///
    /// The refusal names `T` and drops the parser's own reason, inline or from
    /// a file alike: `T::Err`'s `Display` is free to quote its input.
    #[expect(
        clippy::map_err_ignore,
        reason = "the parser's error may quote the value; the refusal names the type instead"
    )]
    pub fn parse<T: FromStr>(&self) -> Result<T, ConfigError> {
        self.value.parse::<T>().map_err(|_| {
            self.refuse(format_args!(
                "does not parse as {}",
                std::any::type_name::<T>()
            ))
        })
    }

    /// The value decoded from JSON as `T`, refused under this setting's variable
    /// when it does not decode.
    ///
    /// The refusal is [`ConfigError::Decode`], worded by [`DecodeError`] for
    /// both spellings: where in the value it failed, never the value.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, ConfigError> {
        serde_json::from_str(&self.value).map_err(|error| ConfigError::Decode {
            var: self.var.clone(),
            source: DecodeError::new(&error)
                .located::<T, _>(&mut serde_json::Deserializer::from_str(&self.value)),
        })
    }
}

impl<T> fmt::Debug for Setting<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setting")
            .field("var", &self.var)
            .field("from_file", &self.from_file)
            .finish_non_exhaustive()
    }
}

/// A value one variable holds, read from its text — what a `#[config]` field
/// of this type, an `Option` of it or a comma-separated list of it reads
/// ([`ConfigField`](crate::ConfigField)).
///
/// The framework reads the standard scalars, `bool` in the vocabulary
/// [`ConfigService::flag`](crate::ConfigService::flag) accepts, and a
/// [`Secret`](nest_rs_core::Secret) of any of them; a type of your own says
/// what it expects:
///
/// ```
/// use std::borrow::Cow;
/// use nest_rs_config::FromSetting;
///
/// pub enum Tier { Free, Paid }
///
/// impl FromSetting for Tier {
///     fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
///         match text {
///             "free" => Ok(Self::Free),
///             "paid" => Ok(Self::Paid),
///             _ => Err("is neither `free` nor `paid`".into()),
///         }
///     }
/// }
/// assert!(Tier::from_setting("gold").is_err());
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is no value a `#[config]` field reads from one variable",
    label = "no `FromSetting` reads this type",
    note = "implement `FromSetting` for it, saying what a refused value was expected to be; a \
            `Duration` is read through its bounds, `#[config(bounds = …)]`, and a structured \
            value as JSON, `#[config(json)]`"
)]
pub trait FromSetting: Sized {
    /// The value `text` holds, or what was expected of it: the refusal names
    /// the variable and ends with this sentence, so it never quotes `text`,
    /// which may be a secret read from a file.
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>>;
}

/// [`FromStr`], its own reason dropped — `T::Err`'s `Display` is free to quote
/// its input — for the type it names.
#[expect(
    clippy::map_err_ignore,
    reason = "the parser's error may quote the value; the refusal names the type instead"
)]
fn parsed<T: FromStr>(text: &str) -> Result<T, Cow<'static, str>> {
    text.parse::<T>()
        .map_err(|_| format!("does not parse as {}", std::any::type_name::<T>()).into())
}

macro_rules! from_str_settings {
    ($($ty:ty),* $(,)?) => {
        $(
            impl FromSetting for $ty {
                fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
                    parsed(text)
                }
            }
        )*
    };
}

from_str_settings!(
    String,
    std::path::PathBuf,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
);

macro_rules! address_settings {
    ($($ty:ty => $expected:literal),* $(,)?) => {
        $(
            impl FromSetting for $ty {
                fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
                    text.parse().map_err(|_| Cow::Borrowed($expected))
                }
            }
        )*
    };
}

address_settings!(
    std::net::IpAddr => "is not an IP address",
    std::net::Ipv4Addr => "is not an IPv4 address",
    std::net::Ipv6Addr => "is not an IPv6 address",
    std::net::SocketAddr => "is not a socket address, `host:port`",
);

/// `1`/`true`/`yes`/`on` and their negatives, whatever their case — the
/// vocabulary [`ConfigService::flag`](crate::ConfigService::flag) reads.
impl FromSetting for bool {
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
        nest_rs_core::parse_bool(text).ok_or(Cow::Borrowed(
            "expected a boolean (1/0, true/false, yes/no, on/off)",
        ))
    }
}

/// A UUID in any form the `uuid` crate parses — hyphenated, simple, braced or
/// URN — the identifier type the framework's entities and tenants carry.
impl FromSetting for uuid::Uuid {
    #[expect(
        clippy::map_err_ignore,
        reason = "the parser's error says where the text broke, and the sentence says enough"
    )]
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
        Self::parse_str(text).map_err(|_| Cow::Borrowed("is not a UUID"))
    }
}

/// The value `T` reads, held so that `Debug` never shows it.
impl<T: FromSetting> FromSetting for nest_rs_core::Secret<T> {
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
        T::from_setting(text).map(Self::new)
    }
}

impl Setting {
    /// The value read as `T`, refused under this setting's variable with what
    /// `T` expected.
    pub(crate) fn read<T: FromSetting>(&self) -> Result<T, ConfigError> {
        T::from_setting(&self.value).map_err(|expected| self.refuse(expected))
    }
}
