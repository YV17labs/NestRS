//! [`Setting`] — a value as the deployment gave it, and the spelling that
//! supplied it.

use std::fmt;
use std::str::FromStr;

use nest_rs_core::DecodeError;
use serde::de::DeserializeOwned;

use crate::error::ConfigError;

/// One key's value and the variable that supplied it: `<KEY>` when it was
/// inline, `<KEY>_FILE` when it was read from the file that variable names.
///
/// It exists so a consumer that judges a value words its refusal through
/// [`refuse`](Self::refuse), which names the spelling the deployment actually
/// set, and quotes the value only through [`shown`](Self::shown), which never
/// repeats what a file held — `_FILE` is the secrets channel, and a boot error
/// lands in every log and CI transcript that captures it.
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
    /// supplied it. `reason` must not quote the value; quote it through
    /// [`shown`](Setting::shown).
    pub fn refuse(&self, reason: impl fmt::Display) -> ConfigError {
        ConfigError::parse(self.var.clone(), reason.to_string())
    }
}

impl Setting {
    /// The value parsed as `T`, refused under this setting's variable when it
    /// does not parse.
    ///
    /// The parser's own reason is kept for an inline value and dropped for one
    /// read from a file: `T::Err`'s `Display` is free to quote its input, and a
    /// file is where a secret is given.
    pub fn parse<T>(&self) -> Result<T, ConfigError>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        self.value.parse::<T>().map_err(|e| {
            if self.from_file {
                self.refuse(format_args!(
                    "does not parse as {}",
                    std::any::type_name::<T>()
                ))
            } else {
                self.refuse(e)
            }
        })
    }

    /// The value decoded from JSON as `T`, refused under this setting's variable
    /// when it does not decode.
    ///
    /// The refusal is [`ConfigError::Decode`], worded by
    /// [`DecodeError`] for both spellings: where the value failed, the kind of
    /// value found and the type expected, never the value — unlike
    /// [`parse`](Self::parse), which keeps an inline parser's reason, since a
    /// structured value is where records carrying credentials are written.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, ConfigError> {
        serde_json::from_str(&self.value).map_err(|error| ConfigError::Decode {
            var: self.var.clone(),
            source: DecodeError::new(&error),
        })
    }

    /// The value as a refusal may quote it: verbatim when it was inline, and a
    /// fixed placeholder naming the variable when it was read from a file.
    pub fn shown(&self) -> impl fmt::Display + '_ {
        Shown(self)
    }
}

struct Shown<'a>(&'a Setting);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.from_file {
            write!(f, "<the content of the file {} names>", self.0.var)
        } else {
            f.write_str(&self.0.value)
        }
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
