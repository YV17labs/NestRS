//! A serde value form-encoded (`application/x-www-form-urlencoded`): a
//! form's fields, or one query parameter, as poem's client wrote them through
//! `serde_urlencoded` — in the order they serialize, a `None` left out, a
//! float written whole.

use serde::Serialize;
use serde::de::value::Error;
use serde::ser::{self, Error as _};
use url::form_urlencoded;

/// `form` — a struct, a map or a sequence of `(name, value)` pairs —
/// form-encoded, its fields in the order they serialize; a field holding
/// `None` is left out.
///
/// # Panics
///
/// When `form` is none of those, or a field holds anything but a scalar or an
/// `Option` of one, naming the field.
pub(crate) fn encode_form(form: &impl Serialize) -> String {
    let fields = match form.serialize(Reader) {
        Ok(Part::Entries(_, fields)) => fields,
        Ok(Part::Items(_, items)) => items.into_iter().map(pair).collect(),
        Ok(other) => panic!(
            "a form is a struct, a map or a sequence of pairs, not {}",
            other.kind()
        ),
        Err(error) => panic!("the form does not serialize: {error}"),
    };
    let mut body = form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        let Part::Scalar(name) = name else {
            panic!("a form field's name is a scalar, not {}", name.kind());
        };
        if let Some(value) = scalar(value, || format!("the form field `{name}`")) {
            body.append_pair(&name, &value);
        }
    }
    body.finish()
}

/// Appends the parameter `name`, holding `value`, to the form-encoded
/// `query`; nothing when `value` is `None`.
///
/// # Panics
///
/// When `value` is anything but a scalar or an `Option` of one, naming the
/// parameter.
pub(crate) fn append_parameter(query: &mut String, name: &str, value: &impl Serialize) {
    let subject = || format!("the query parameter `{name}`");
    let value = value
        .serialize(Reader)
        .unwrap_or_else(|error| panic!("{} does not serialize: {error}", subject()));
    if let Some(value) = scalar(value, subject) {
        form_urlencoded::Serializer::for_suffix(query, 0).append_pair(name, &value);
    }
}

/// One `(name, value)` pair of a form written as a sequence.
fn pair(item: Part) -> (Part, Part) {
    match item {
        Part::Items(_, items) => <[Part; 2]>::try_from(items)
            .map(|[name, value]| (name, value))
            .unwrap_or_else(|items| {
                panic!(
                    "a form written as a sequence holds pairs, not {} items",
                    items.len()
                )
            }),
        other => panic!(
            "a form written as a sequence holds pairs, not {}",
            other.kind()
        ),
    }
}

/// `value` as a field holds it: its text, or nothing for `None`.
///
/// # Panics
///
/// When it is neither, naming the field `subject` says.
fn scalar(value: Part, subject: impl FnOnce() -> String) -> Option<String> {
    match value {
        Part::Scalar(text) => Some(text),
        Part::Nothing => None,
        other => panic!(
            "{} holds {}: a field is a string, a number, a boolean or an `Option` of one",
            subject(),
            other.kind()
        ),
    }
}

/// A value as serde's data model says it, in the order it says it.
enum Part {
    /// A boolean, a number, a character, a string, a unit variant or a unit
    /// struct's name, as a field writes it.
    Scalar(String),
    /// `None`.
    Nothing,
    /// A sequence's or a tuple's items, under its kind.
    Items(&'static str, Vec<Part>),
    /// A map's or a struct's entries, under its kind.
    Entries(&'static str, Vec<(Part, Part)>),
    /// What no form holds — a unit, an enum variant with data, bytes that are
    /// not UTF-8 — under its kind.
    Other(&'static str),
}

impl Part {
    /// What kind of value this is, as a sentence says it.
    fn kind(&self) -> &'static str {
        match self {
            Self::Scalar(_) => "a scalar",
            Self::Nothing => "`None`",
            Self::Items(kind, _) | Self::Entries(kind, _) | Self::Other(kind) => kind,
        }
    }
}

/// A finite float in its shortest round-trip form, as poem's encoder and JSON
/// both write it.
fn finite(value: &impl Serialize) -> Part {
    Part::Scalar(serde_json::to_string(value).expect("a finite float is a JSON number"))
}

/// The serializer reading a value into its [`Part`]; it refuses nothing, so
/// its one error is a value's own `Serialize` failing.
struct Reader;

impl ser::Serializer for Reader {
    type Ok = Part;
    type Error = Error;
    type SerializeSeq = Items;
    type SerializeTuple = Items;
    type SerializeTupleStruct = Items;
    type SerializeTupleVariant = Variant;
    type SerializeMap = Entries;
    type SerializeStruct = Entries;
    type SerializeStructVariant = Variant;

    fn serialize_bool(self, value: bool) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_i8(self, value: i8) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_i16(self, value: i16) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_i32(self, value: i32) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_i64(self, value: i64) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_i128(self, value: i128) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_u8(self, value: u8) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_u16(self, value: u16) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_u32(self, value: u32) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_u64(self, value: u64) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_u128(self, value: u128) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_f32(self, value: f32) -> Result<Part, Error> {
        if value.is_finite() {
            Ok(finite(&value))
        } else {
            self.serialize_f64(f64::from(value))
        }
    }

    fn serialize_f64(self, value: f64) -> Result<Part, Error> {
        if value.is_finite() {
            return Ok(finite(&value));
        }
        let text = if value.is_nan() {
            "NaN"
        } else if value.is_sign_negative() {
            "-inf"
        } else {
            "inf"
        };
        Ok(Part::Scalar(text.to_owned()))
    }

    fn serialize_char(self, value: char) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Part, Error> {
        Ok(Part::Scalar(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Part, Error> {
        Ok(
            std::str::from_utf8(value).map_or(Part::Other("bytes that are not UTF-8"), |text| {
                Part::Scalar(text.to_owned())
            }),
        )
    }

    fn serialize_none(self) -> Result<Part, Error> {
        Ok(Part::Nothing)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Part, Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Part, Error> {
        Ok(Part::Other("a unit (`()`, or JSON's `null`)"))
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Part, Error> {
        Ok(Part::Scalar(name.to_owned()))
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Part, Error> {
        Ok(Part::Scalar(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Part, Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Part, Error> {
        Ok(Variant.part())
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Items, Error> {
        Ok(Items::new("a sequence", len.unwrap_or_default()))
    }

    fn serialize_tuple(self, len: usize) -> Result<Items, Error> {
        Ok(Items::new("a tuple", len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Items, Error> {
        Ok(Items::new("a tuple struct", len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Variant, Error> {
        Ok(Variant)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Entries, Error> {
        Ok(Entries::new("a map", len.unwrap_or_default()))
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Entries, Error> {
        Ok(Entries::new("a struct", len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Variant, Error> {
        Ok(Variant)
    }
}

/// A sequence's, a tuple's or a tuple struct's items, read in order.
struct Items {
    kind: &'static str,
    items: Vec<Part>,
}

impl Items {
    fn new(kind: &'static str, len: usize) -> Self {
        Self {
            kind,
            items: Vec::with_capacity(len),
        }
    }

    fn push<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.items.push(value.serialize(Reader)?);
        Ok(())
    }
}

impl ser::SerializeSeq for Items {
    type Ok = Part;
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Part, Error> {
        Ok(Part::Items(self.kind, self.items))
    }
}

impl ser::SerializeTuple for Items {
    type Ok = Part;
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Part, Error> {
        Ok(Part::Items(self.kind, self.items))
    }
}

impl ser::SerializeTupleStruct for Items {
    type Ok = Part;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Part, Error> {
        Ok(Part::Items(self.kind, self.items))
    }
}

/// A map's or a struct's entries, read in order.
struct Entries {
    kind: &'static str,
    entries: Vec<(Part, Part)>,
    key: Option<Part>,
}

impl Entries {
    fn new(kind: &'static str, len: usize) -> Self {
        Self {
            kind,
            entries: Vec::with_capacity(len),
            key: None,
        }
    }
}

impl ser::SerializeMap for Entries {
    type Ok = Part;
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        self.key = Some(key.serialize(Reader)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error::custom("a map's value came before its key"))?;
        self.entries.push((key, value.serialize(Reader)?));
        Ok(())
    }

    fn end(self) -> Result<Part, Error> {
        Ok(Part::Entries(self.kind, self.entries))
    }
}

impl ser::SerializeStruct for Entries {
    type Ok = Part;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.entries
            .push((Part::Scalar(name.to_owned()), value.serialize(Reader)?));
        Ok(())
    }

    fn end(self) -> Result<Part, Error> {
        Ok(Part::Entries(self.kind, self.entries))
    }
}

/// An enum variant with data, whose fields no form reads.
struct Variant;

impl Variant {
    fn part(self) -> Part {
        Part::Other("an enum variant with data")
    }
}

impl ser::SerializeTupleVariant for Variant {
    type Ok = Part;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, _value: &T) -> Result<(), Error> {
        Ok(())
    }

    fn end(self) -> Result<Part, Error> {
        Ok(self.part())
    }
}

impl ser::SerializeStructVariant for Variant {
    type Ok = Part;
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _name: &'static str,
        _value: &T,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn end(self) -> Result<Part, Error> {
        Ok(self.part())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Serialize)]
    struct Token {
        grant_type: &'static str,
        code: &'static str,
        state: Option<&'static str>,
    }

    #[test]
    fn a_struct_s_fields_go_in_declaration_order_and_a_none_is_left_out() {
        let token = Token {
            grant_type: "authorization_code",
            code: "a b&c",
            state: None,
        };
        assert_eq!(
            encode_form(&token),
            "grant_type=authorization_code&code=a+b%26c"
        );
    }

    #[test]
    fn a_form_is_also_a_map_or_a_sequence_of_pairs() {
        let map = BTreeMap::from([("b", 2), ("a", 1)]);
        assert_eq!(encode_form(&map), "a=1&b=2");
        assert_eq!(encode_form(&[("z", "1"), ("a", "x y")]), "z=1&a=x+y");
        assert_eq!(
            encode_form(&vec![("cursor", None), ("q", Some("rust"))]),
            "q=rust"
        );
    }

    #[test]
    fn a_scalar_is_written_as_poem_wrote_it() {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Sort {
            NewestFirst,
        }
        #[derive(Serialize)]
        struct Id(u64);
        #[derive(Serialize)]
        struct Reading {
            sensor: char,
            on: bool,
            id: Id,
            sort: Sort,
            ratio: f32,
            mean: f64,
            low: f64,
            high: f64,
            missing: f64,
        }
        let reading = Reading {
            sensor: 't',
            on: true,
            id: Id(7),
            sort: Sort::NewestFirst,
            ratio: 0.1,
            mean: 2.0,
            low: f64::NEG_INFINITY,
            high: f64::INFINITY,
            missing: f64::NAN,
        };
        assert_eq!(
            encode_form(&reading),
            "sensor=t&on=true&id=7&sort=newest_first&ratio=0.1&mean=2.0&low=-inf&high=inf&missing=NaN"
        );
    }

    #[test]
    fn a_parameter_is_appended_after_what_the_query_holds() {
        let mut query = String::from("scope=all");
        append_parameter(&mut query, "q", &"café & co");
        append_parameter(&mut query, "cursor", &None::<u32>);
        append_parameter(&mut query, "page", &Some(2));
        assert_eq!(query, "scope=all&q=caf%C3%A9+%26+co&page=2");
    }

    #[test]
    #[should_panic(
        expected = "the form field `tags` holds a sequence: a field is a string, a number, a boolean or an `Option` of one"
    )]
    fn a_nested_form_field_panics_naming_it() {
        let _ = encode_form(&serde_json::json!({ "tags": ["a", "b"] }));
    }

    #[test]
    #[should_panic(expected = "the query parameter `filter` holds a struct")]
    fn a_nested_parameter_panics_naming_it() {
        append_parameter(
            &mut String::new(),
            "filter",
            &Token {
                grant_type: "x",
                code: "y",
                state: None,
            },
        );
    }

    #[test]
    #[should_panic(expected = "a form is a struct, a map or a sequence of pairs, not a scalar")]
    fn a_scalar_is_no_form() {
        let _ = encode_form(&"grant_type=x");
    }

    #[test]
    #[should_panic(expected = "a form written as a sequence holds pairs, not 3 items")]
    fn a_sequence_form_holds_pairs() {
        let _ = encode_form(&[("a", "1", "2")]);
    }
}
