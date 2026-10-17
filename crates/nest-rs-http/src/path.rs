//! [`Path`]: the route's parameters read into a type — one value, a tuple by
//! position, a struct by name.

// The deserializer's shapes and its tests are ported from poem 3.1.12
// (`src/web/path/de.rs`, https://github.com/poem-web/poem), under its MIT
// license, whose notice follows:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use serde::de::{
    DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde::{Deserializer, forward_to_deserialize_any};

use crate::error::{PathError, ResponseError, Result};
use crate::request::{FromRequest, Request, RequestBody};

/// The route's path parameters, each percent-decoded once by the router, read
/// into `T`: one value when the route declares one parameter, a tuple by
/// position, a struct by name.
///
/// A parameter whose text is not what its type reads answers `400` naming the
/// parameter and what was expected, never its value; a handler reading a
/// parameter its route does not declare, or another number of them, is a
/// handler-signature mistake and answers `500`.
///
/// ```
/// use nest_rs_http::{FromRequest, HttpRoutes, Method, Path, Response, endpoint_fn};
///
/// fn mount(routes: &mut HttpRoutes) {
///     routes.route(
///         Method::GET,
///         "/orgs/{org}/members/{id}",
///         endpoint_fn(|request| async move {
///             let (request, mut body) = request.split();
///             let Path((org, id)) =
///                 Path::<(String, u64)>::from_request(&request, &mut body).await?;
///             Ok(Response::builder().body(format!("{org} #{id}")))
///         }),
///     );
/// }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

impl<T> Path<T> {
    /// The value, out of its wrapper.
    ///
    /// ```
    /// use nest_rs_http::Path;
    ///
    /// assert_eq!(Path(7_u64).into_inner(), 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Path<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: DeserializeOwned> FromRequest<'a> for Path<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        read(req.path_params()).map(Path).map_err(|refused| {
            if refused.status().is_server_error() {
                tracing::error!(
                    target: crate::target::HTTP,
                    error = %nest_rs_core::error_message(&refused),
                    "a handler reads path parameters its route cannot bind",
                );
            }
            refused.into()
        })
    }
}

/// `T` read from `params`; a refusal no parameter claimed is the one
/// parameter's when the route declares one — a type refusing after it read
/// its text.
fn read<T: DeserializeOwned>(params: &[Param]) -> Result<T, PathError> {
    T::deserialize(Params(params)).map_err(|refused| match params {
        [(name, _)] => refused.against(name),
        _ => refused,
    })
}

/// One parameter's name and decoded value, as the router recorded it.
type Param = (Arc<str>, String);

/// Every parameter of the matched route, in its template's order.
struct Params<'de>(&'de [Param]);

impl<'de> Params<'de> {
    /// The one parameter a single value reads.
    fn single(&self) -> Result<Value<'de>, PathError> {
        match self.0 {
            [param] => Ok(Value::of(param)),
            params => Err(PathError::Arity {
                declared: params.len(),
                read: 1,
            }),
        }
    }

    /// Exactly `read` parameters, for a tuple of that many: a shorter one
    /// would bind the route's first parameters in place of the ones it means.
    fn exactly(&self, read: usize) -> Result<(), PathError> {
        if self.0.len() != read {
            return Err(PathError::Arity {
                declared: self.0.len(),
                read,
            });
        }
        Ok(())
    }
}

/// A single value's scalar arms: the one parameter, parsed by its value's arm.
macro_rules! single_arms {
    ($($method:ident),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
                let value = self.single()?;
                let name = value.name;
                value.$method(visitor).map_err(|refused| refused.against(name))
            }
        )*
    };
}

impl<'de> Deserializer<'de> for Params<'de> {
    type Error = PathError;

    single_arms! {
        deserialize_bool, deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64,
        deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32, deserialize_u64,
        deserialize_u128, deserialize_f32, deserialize_f64, deserialize_char, deserialize_str,
        deserialize_string, deserialize_byte_buf,
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a self-describing value"))
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("bytes"))
    }

    fn deserialize_option<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("an `Option`"))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("an identifier"))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_seq(Positions(self.0))
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        self.exactly(len)?;
        visitor.visit_seq(Positions(self.0))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_map(Names {
            params: self.0,
            value: None,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, PathError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, PathError> {
        let value = self.single()?;
        let param = value.name;
        value
            .deserialize_enum(name, variants, visitor)
            .map_err(|refused| refused.against(param))
    }
}

/// The parameters by position, for a tuple or a sequence.
struct Positions<'de>(&'de [Param]);

impl<'de> SeqAccess<'de> for Positions<'de> {
    type Error = PathError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, PathError> {
        let Some((param, rest)) = self.0.split_first() else {
            return Ok(None);
        };
        self.0 = rest;
        Value::of(param).read(seed).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

/// The parameters by name, for a struct or a map.
struct Names<'de> {
    params: &'de [Param],
    value: Option<&'de Param>,
}

impl<'de> MapAccess<'de> for Names<'de> {
    type Error = PathError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, PathError> {
        let Some((param, rest)) = self.params.split_first() else {
            return Ok(None);
        };
        self.params = rest;
        self.value = Some(param);
        seed.deserialize(Key(&param.0)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, PathError> {
        // serde's contract asks for a value only after its key.
        let Some(param) = self.value.take() else {
            return Err(PathError::Refused);
        };
        Value::of(param).read(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.params.len())
    }
}

/// A parameter's name, as a struct's field or a map's key reads it.
struct Key<'de>(&'de str);

impl<'de> Deserializer<'de> for Key<'de> {
    type Error = PathError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_borrowed_str(self.0)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map
        struct enum identifier ignored_any
    }
}

/// One parameter's value, deserialized into its field's type; a refusal is
/// attributed to the parameter, never said with its text.
struct Value<'de> {
    name: &'de str,
    text: &'de str,
}

impl<'de> Value<'de> {
    fn of(param: &'de Param) -> Self {
        Self {
            name: &param.0,
            text: &param.1,
        }
    }

    /// `seed` read from this value, its refusal named against the parameter.
    fn read<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, PathError> {
        let name = self.name;
        seed.deserialize(self)
            .map_err(|refused| refused.against(name))
    }
}

/// The scalar arms, each parsing the parameter's text into the field's type and
/// reporting the kind it expected — never the text it read.
macro_rules! parse_arms {
    ($($method:ident => $visit:ident, $ty:ty, $expected:literal;)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
                match self.text.parse::<$ty>() {
                    Ok(value) => visitor.$visit(value),
                    Err(_) => Err(PathError::malformed(self.name, $expected)),
                }
            }
        )*
    };
}

impl<'de> Deserializer<'de> for Value<'de> {
    type Error = PathError;

    parse_arms! {
        deserialize_bool => visit_bool, bool, "a boolean";
        deserialize_i8 => visit_i8, i8, "an integer";
        deserialize_i16 => visit_i16, i16, "an integer";
        deserialize_i32 => visit_i32, i32, "an integer";
        deserialize_i64 => visit_i64, i64, "an integer";
        deserialize_i128 => visit_i128, i128, "an integer";
        deserialize_u8 => visit_u8, u8, "an integer";
        deserialize_u16 => visit_u16, u16, "an integer";
        deserialize_u32 => visit_u32, u32, "an integer";
        deserialize_u64 => visit_u64, u64, "an integer";
        deserialize_u128 => visit_u128, u128, "an integer";
        deserialize_f32 => visit_f32, f32, "a number";
        deserialize_f64 => visit_f64, f64, "a number";
        deserialize_char => visit_char, char, "a single character";
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a self-describing value"))
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_borrowed_str(self.text)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_borrowed_str(self.text)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_borrowed_bytes(self.text.as_bytes())
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_borrowed_bytes(self.text.as_bytes())
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, PathError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a list"))
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a tuple"))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a tuple"))
    }

    fn deserialize_map<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a map"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a struct"))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("an identifier"))
    }

    /// An enum-typed parameter: its text names a unit variant.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, PathError> {
        visitor.visit_enum(Variant(self.text))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PathError> {
        visitor.visit_unit()
    }
}

/// A parameter naming a unit variant.
struct Variant<'de>(&'de str);

impl<'de> EnumAccess<'de> for Variant<'de> {
    type Error = PathError;
    type Variant = UnitOnly;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, UnitOnly), PathError> {
        Ok((seed.deserialize(Key(self.0))?, UnitOnly))
    }
}

/// What a variant read from one segment can be: a unit, nothing it carries.
struct UnitOnly;

impl<'de> VariantAccess<'de> for UnitOnly {
    type Error = PathError;

    fn unit_variant(self) -> Result<(), PathError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        _seed: T,
    ) -> Result<T::Value, PathError> {
        Err(PathError::Unsupported("a variant carrying a value"))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a variant carrying a value"))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, PathError> {
        Err(PathError::Unsupported("a variant carrying a value"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde::de::Error as _;

    use super::*;

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    enum MyEnum {
        A,
        B,
        #[serde(rename = "c")]
        C,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct Struct {
        c: String,
        b: bool,
        a: i32,
    }

    fn params(values: &[(&str, &str)]) -> Vec<Param> {
        values
            .iter()
            .map(|(name, value)| (Arc::from(*name), (*value).to_owned()))
            .collect()
    }

    fn read<T: DeserializeOwned>(values: &[(&str, &str)]) -> Result<T, PathError> {
        super::read(&params(values))
    }

    macro_rules! check_single_value {
        ($ty:ty, $text:literal, $value:expr) => {
            assert_eq!(read::<$ty>(&[("value", $text)]).unwrap(), $value);
        };
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "the parsed float is exact")]
    fn test_parse_single_value() {
        check_single_value!(bool, "true", true);
        check_single_value!(bool, "false", false);
        check_single_value!(i8, "-123", -123);
        check_single_value!(i16, "-123", -123);
        check_single_value!(i32, "-123", -123);
        check_single_value!(i64, "-123", -123);
        check_single_value!(u8, "123", 123);
        check_single_value!(u16, "123", 123);
        check_single_value!(u32, "123", 123);
        check_single_value!(u64, "123", 123);
        check_single_value!(f32, "123", 123.0);
        check_single_value!(f64, "123", 123.0);
        check_single_value!(String, "abc", "abc");
        check_single_value!(char, "a", 'a');

        assert_eq!(read::<MyEnum>(&[("a", "B")]).unwrap(), MyEnum::B);

        assert!(matches!(
            read::<i32>(&[("a", "1"), ("b", "2")]).unwrap_err(),
            PathError::Arity {
                declared: 2,
                read: 1
            }
        ));
    }

    #[test]
    fn test_parse_seq() {
        let values = [("a", "1"), ("b", "true"), ("c", "abc")];
        assert_eq!(
            read::<(i32, bool, String)>(&values).unwrap(),
            (1, true, "abc".to_owned())
        );

        #[derive(Debug, Deserialize, Eq, PartialEq)]
        struct TupleStruct(i32, bool, String);
        assert_eq!(
            read::<TupleStruct>(&values).unwrap(),
            TupleStruct(1, true, "abc".to_owned())
        );

        assert_eq!(
            read::<Vec<i32>>(&[("a", "1"), ("b", "2"), ("c", "3")]).unwrap(),
            vec![1, 2, 3]
        );

        assert_eq!(
            read::<Vec<MyEnum>>(&[("a", "c"), ("a", "B")]).unwrap(),
            vec![MyEnum::C, MyEnum::B]
        );
    }

    #[test]
    fn test_parse_struct() {
        assert_eq!(
            read::<Struct>(&[("a", "1"), ("b", "true"), ("c", "abc")]).unwrap(),
            Struct {
                c: "abc".to_owned(),
                b: true,
                a: 1,
            }
        );
    }

    #[test]
    fn test_parse_map() {
        assert_eq!(
            read::<HashMap<String, String>>(&[("a", "1"), ("b", "true"), ("c", "abc")]).unwrap(),
            [("a", "1"), ("b", "true"), ("c", "abc")]
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect()
        );
    }

    #[test]
    fn a_value_its_type_refuses_names_the_parameter_and_what_was_expected() {
        let refused = read::<(String, u64)>(&[("org", "acme"), ("id", "sk_live_51")]).unwrap_err();
        assert_eq!(refused.to_string(), "path parameter `id` is not an integer");

        let refused = read::<MyEnum>(&[("kind", "sk_live_51")]).unwrap_err();
        let said = refused.to_string();
        assert!(
            said.starts_with("path parameter `kind` is not one of"),
            "{said}"
        );
        assert!(!said.contains("sk_live"), "{said}");

        #[derive(Debug)]
        struct Even;
        impl<'de> Deserialize<'de> for Even {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = <&str>::deserialize(d)?;
                Err(D::Error::custom(format!("`{text}` is odd")))
            }
        }
        let refused = read::<Even>(&[("n", "sk_live_51")]).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "path parameter `n` is not a value its type accepts"
        );
    }

    #[test]
    fn a_handler_reading_what_its_route_does_not_declare_is_a_signature_mistake() {
        #[derive(Debug, Deserialize)]
        struct Wants {
            #[expect(dead_code, reason = "read only to be refused")]
            missing: String,
        }
        let refused = read::<Wants>(&[("id", "1")]).unwrap_err();
        assert!(
            matches!(refused, PathError::Undeclared(ref name) if name == "missing"),
            "{refused}"
        );

        assert!(matches!(
            read::<(u64, u64)>(&[("id", "1")]).unwrap_err(),
            PathError::Arity {
                declared: 1,
                read: 2
            }
        ));
        assert!(matches!(
            read::<u64>(&[]).unwrap_err(),
            PathError::Arity {
                declared: 0,
                read: 1
            }
        ));
        assert!(matches!(
            read::<Option<u64>>(&[("id", "1")]).unwrap_err(),
            PathError::Unsupported(_)
        ));
    }

    #[test]
    fn a_tuple_reads_exactly_the_declared_parameters() {
        let route = [("org", "1"), ("team", "2"), ("id", "3")];
        assert!(matches!(
            read::<(u64, u64)>(&route).unwrap_err(),
            PathError::Arity {
                declared: 3,
                read: 2
            }
        ));
        assert!(matches!(
            read::<(u64,)>(&route[..2]).unwrap_err(),
            PathError::Arity {
                declared: 2,
                read: 1
            }
        ));

        #[derive(Debug, Deserialize)]
        struct OrgAndId(
            #[expect(dead_code, reason = "read only to be refused")] u64,
            #[expect(dead_code, reason = "read only to be refused")] u64,
        );
        assert!(matches!(
            read::<OrgAndId>(&route).unwrap_err(),
            PathError::Arity {
                declared: 3,
                read: 2
            }
        ));

        assert_eq!(read::<(u64, u64, u64)>(&route).unwrap(), (1, 2, 3));
        assert_eq!(read::<Vec<u64>>(&route).unwrap(), [1, 2, 3]);
    }
}
