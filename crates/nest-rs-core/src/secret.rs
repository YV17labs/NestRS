//! [`Secret`] — a value no format, log line or serialized reply prints.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What a [`Secret`] says in place of its value, in `Debug` and when serialized.
const REDACTED: &str = "<redacted>";

/// A value no `Debug`, log line or serialized reply prints — a password hash, a
/// client secret, a signing key, a URL carrying a password, the email a social
/// provider reported.
///
/// `Debug` writes `<redacted>`, there is no `Display`, and serializing writes the
/// string `"<redacted>"`, so a struct holding one derives both and keeps the
/// value to itself; [`expose_secret`](Self::expose_secret) is the one read, at
/// the call that needs the value. Deserializing reads the value itself, so a
/// configuration or a row decodes into one, and `==` between two byte-like
/// secrets runs in constant time. With `nest-rs-seaorm`, it is a column type
/// over its value's own, and `#[expose]` refuses it on the wire.
///
/// ```
/// use nest_rs_core::Secret;
///
/// #[derive(Debug)]
/// struct Client {
///     id: String,
///     secret: Secret<String>,
/// }
///
/// let client = Client { id: "ci".into(), secret: "s3cr3t".into() };
/// assert_eq!(format!("{client:?}"), r#"Client { id: "ci", secret: <redacted> }"#);
/// assert_eq!(client.secret.expose_secret(), "s3cr3t");
/// ```
#[derive(Clone, Default)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// `value`, kept out of every format from here on.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// The value itself, for the call that needs it: a hash to verify, a key to
    /// sign with, a URL to dial.
    pub const fn expose_secret(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret<String> {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// Constant-time over the bytes: how long `==` takes says nothing of where two
/// secrets of one length differ.
impl<T: AsRef<[u8]>> PartialEq for Secret<T> {
    fn eq(&self, other: &Self) -> bool {
        constant_time_eq::constant_time_eq(self.0.as_ref(), other.0.as_ref())
    }
}

impl<T: AsRef<[u8]>> Eq for Secret<T> {}

/// The placeholder, never the value: a reply, a stored document or a validation
/// error's parameters built by serializing a struct carries `"<redacted>"`.
impl<T> Serialize for Secret<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

/// `#[validate(length(…))]` judges the value's length; the refusal's `value`
/// parameter is the placeholder.
impl<T: validator::ValidateLength<u64>> validator::ValidateLength<u64> for Secret<T> {
    fn length(&self) -> Option<u64> {
        self.0.length()
    }
}

/// A column holding a secret, stored and read as its value's own type.
#[cfg(feature = "seaorm")]
mod column {
    use sea_orm::sea_query::{ArrayType, ColumnType, Nullable, ValueType, ValueTypeErr};
    use sea_orm::{
        ActiveValue, ColIdx, IntoActiveValue, QueryResult, TryFromU64, TryGetError, TryGetable,
        Value,
    };

    use super::Secret;

    impl<T: Into<Value>> From<Secret<T>> for Value {
        fn from(secret: Secret<T>) -> Self {
            secret.0.into()
        }
    }

    impl<T: TryGetable> TryGetable for Secret<T> {
        fn try_get_by<I: ColIdx>(res: &QueryResult, index: I) -> Result<Self, TryGetError> {
            T::try_get_by(res, index).map(Secret)
        }
    }

    impl<T: ValueType> ValueType for Secret<T> {
        fn try_from(value: Value) -> Result<Self, ValueTypeErr> {
            T::try_from(value).map(Secret)
        }

        fn type_name() -> String {
            format!("Secret<{}>", T::type_name())
        }

        fn array_type() -> ArrayType {
            T::array_type()
        }

        fn column_type() -> ColumnType {
            T::column_type()
        }

        fn enum_type_name() -> Option<&'static str> {
            T::enum_type_name()
        }
    }

    impl<T: Nullable> Nullable for Secret<T> {
        fn null() -> Value {
            T::null()
        }
    }

    impl<T: Into<Value>> IntoActiveValue<Self> for Secret<T> {
        fn into_active_value(self) -> ActiveValue<Self> {
            ActiveValue::Set(self)
        }
    }

    /// A secret primary key, as sea-orm reads one an insert returned.
    impl<T: TryFromU64> TryFromU64 for Secret<T> {
        fn try_from_u64(n: u64) -> Result<Self, sea_orm::DbErr> {
            T::try_from_u64(n).map(Secret)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Secret<String> {
        Secret::from(value)
    }

    #[test]
    fn debug_prints_the_placeholder_alone() {
        #[derive(Debug)]
        #[expect(dead_code, reason = "read through Debug alone")]
        struct Row {
            name: &'static str,
            hash: Option<Secret<String>>,
        }

        let row = Row {
            name: "ada",
            hash: Some(text("$argon2id$v=19$secret")),
        };
        let printed = format!("{row:?} {row:#?}");
        assert!(!printed.contains("argon2id"), "{printed}");
        assert!(printed.contains("hash: Some(<redacted>)"), "{printed}");
        assert!(printed.contains("ada"), "the other fields stay: {printed}");
    }

    #[test]
    fn the_value_is_read_through_expose_secret() {
        let secret = Secret::new(vec![1_u8, 2, 3]);
        assert_eq!(secret.expose_secret(), &[1, 2, 3]);
        assert_eq!(text("key").expose_secret(), "key");
    }

    #[test]
    fn serializing_writes_the_placeholder_and_deserializing_reads_the_value() {
        let written = serde_json::to_value(Some(text("s3cr3t"))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(written, serde_json::json!("<redacted>"));

        let read: Option<Secret<String>> =
            serde_json::from_value(serde_json::json!("s3cr3t")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read.as_ref().map(Secret::expose_secret).map(String::as_str),
            Some("s3cr3t")
        );
        let absent: Option<Secret<String>> =
            serde_json::from_value(serde_json::Value::Null).unwrap_or_else(|e| panic!("{e}"));
        assert!(absent.is_none());
    }

    #[test]
    fn equality_compares_the_bytes() {
        assert_eq!(text("abc"), text("abc"));
        assert_ne!(text("abc"), text("abd"));
        assert_ne!(text("abc"), text("abcd"));
        assert_eq!(Secret::new(Vec::<u8>::new()), Secret::default());
    }

    #[test]
    fn a_refused_length_names_the_placeholder_as_its_value() {
        use validator::Validate;

        #[derive(Validate)]
        #[validate(crate = validator)]
        struct Settings {
            #[validate(length(min = 32))]
            key: Secret<String>,
            #[validate(length(min = 32))]
            spare: Option<Secret<String>>,
        }

        let settings = Settings {
            key: "short-but-secret".into(),
            spare: Some("also-secret".into()),
        };
        let errors = settings.validate().expect_err("both keys are too short");
        let rendered = serde_json::to_string(&errors).unwrap_or_else(|e| panic!("{e}"));
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert_eq!(errors.field_errors().len(), 2, "{rendered}");

        let long = Settings {
            key: "k".repeat(32).as_str().into(),
            spare: None,
        };
        assert!(long.validate().is_ok());
    }

    #[cfg(feature = "seaorm")]
    #[test]
    fn a_column_stores_and_reads_its_value_s_own_type() {
        use sea_orm::Value;
        use sea_orm::sea_query::{ColumnType, Nullable, ValueType};

        assert_eq!(
            Value::from(text("hash")),
            Value::String(Some("hash".to_owned()))
        );
        let read = <Secret<String> as ValueType>::try_from(Value::String(Some("hash".to_owned())))
            .unwrap_or_else(|_| panic!("a string column reads into a secret"));
        assert_eq!(read.expose_secret(), "hash");
        assert!(matches!(
            <Secret<String> as ValueType>::column_type(),
            ColumnType::String(_)
        ));
        assert_eq!(
            <Secret<String> as Nullable>::null(),
            <String as Nullable>::null()
        );
        assert_eq!(<Secret<String> as ValueType>::type_name(), "Secret<String>");
    }
}
