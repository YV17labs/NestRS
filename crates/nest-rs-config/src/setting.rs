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
    /// both spellings, never quoting the value.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, ConfigError> {
        serde_json::from_str(&self.value).map_err(|error| ConfigError::Decode {
            var: self.var.clone(),
            source: DecodeError::new(&error),
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
