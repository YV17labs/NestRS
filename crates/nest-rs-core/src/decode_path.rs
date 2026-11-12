//! Where a payload's decode failed, said in the names its type declares.
//!
//! [`trace`] runs a decode over a [`Tracked`] deserializer, which hands every
//! nested deserializer, visitor and access the step that leads to it. A key is
//! said by the name a struct's `fields` or an enum's `variants` list gives it,
//! never by the payload's text, and any other key as a map entry: a key
//! capture that kept the text, as serde_path_to_error's does, would quote what
//! the client chose.

use std::cell::RefCell;
use std::fmt::{self, Write as _};

use serde::de::{self, Deserialize, DeserializeSeed, Deserializer, Visitor};

/// The names a derive hands its deserializer: a struct's fields, an enum's
/// variants.
type Names = &'static [&'static str];

/// One step from a payload's root towards a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// A field its struct declares, by the name the struct gives it.
    Field(&'static str),
    /// A variant its enum declares.
    Variant(&'static str),
    /// An element of a sequence, by its position.
    Index(usize),
    /// An entry of a map, or a key its struct does not declare: the key is
    /// the payload's, and never said.
    Entry,
}

/// Where a decode failure sits: the steps from the payload's root, rendered
/// as `author.name`, `items[2]`, `limits[…]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecodePath(pub(crate) Vec<Step>);

impl fmt::Display for DecodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, step) in self.0.iter().enumerate() {
            match step {
                Step::Field(name) | Step::Variant(name) => {
                    if at > 0 {
                        f.write_char('.')?;
                    }
                    f.write_str(name)?;
                }
                Step::Index(index) => write!(f, "[{index}]")?,
                Step::Entry => f.write_str("[…]")?,
            }
        }
        Ok(())
    }
}

/// What a tracked decode knows of its failure.
#[derive(Debug, Default)]
pub(crate) struct Failed {
    /// Where it failed.
    pub(crate) path: DecodePath,
    /// The fields of the struct the failure is, when it is a struct's own —
    /// a field missing, a field given twice.
    pub(crate) fields: Option<Names>,
    /// What the visitor that failed expected, in the type's own words.
    pub(crate) expected: Option<String>,
}

/// `T` decoded from `deserializer` under a tracker: a refusal comes back with
/// where it happened.
pub(crate) fn trace<'de, T, D>(deserializer: D) -> Result<T, (D::Error, Failed)>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let track = Track::default();
    T::deserialize(Tracked {
        de: deserializer,
        chain: Chain::Root,
        track: &track,
    })
    .map_err(|error| (error, track.failed.take().unwrap_or_default()))
}

/// The path down to a deserializer, linked through the stack of the calls
/// that reached it.
#[derive(Clone, Copy)]
enum Chain<'a> {
    Root,
    Step { parent: &'a Chain<'a>, step: Step },
}

impl Chain<'_> {
    fn path(&self) -> DecodePath {
        let mut steps = Vec::new();
        let mut at = self;
        while let Chain::Step { parent, step } = at {
            steps.push(*step);
            at = parent;
        }
        steps.reverse();
        DecodePath(steps)
    }
}

/// The failure a decode is meeting, and what each visitor on the way down
/// expects.
#[derive(Default)]
struct Track {
    failed: RefCell<Option<Failed>>,
    expecting: RefCell<Expectations>,
}

/// One `expecting` sentence per depth of the calls under way, each buffer
/// reused by the next call at its depth.
#[derive(Default)]
struct Expectations {
    said: Vec<String>,
    depth: usize,
}

/// A visitor's `expecting`, as `Display`.
struct Expecting<'v, V>(&'v V);

impl<'de, V: Visitor<'de>> fmt::Display for Expecting<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.expecting(f)
    }
}

impl Track {
    /// A deserializer call opens, with what its visitor expects.
    fn enter<'de, V: Visitor<'de>>(&self, visitor: &V) {
        let mut expecting = self.expecting.borrow_mut();
        let depth = expecting.depth;
        if expecting.said.len() == depth {
            expecting.said.push(String::new());
        }
        if let Some(said) = expecting.said.get_mut(depth) {
            said.clear();
            if write!(said, "{}", Expecting(visitor)).is_err() {
                said.clear();
            }
        }
        expecting.depth += 1;
    }

    /// The call `enter` opened returns `outcome`.
    ///
    /// A call that read its value hands it to the visitor of the call around
    /// it, whose own code may still refuse it — a `try_from` newtype, a
    /// validating type — so that visitor's expectation is dropped: what it
    /// expected was met, and a refusal it raises now is its type's alone.
    fn leave<T, E>(
        &self,
        outcome: Result<T, E>,
        chain: &Chain<'_>,
        fields: Option<Names>,
    ) -> Result<T, E> {
        let expected = {
            let mut expecting = self.expecting.borrow_mut();
            expecting.depth = expecting.depth.saturating_sub(1);
            let depth = expecting.depth;
            if outcome.is_ok() {
                if let Some(holder) = depth
                    .checked_sub(1)
                    .and_then(|at| expecting.said.get_mut(at))
                {
                    holder.clear();
                }
                None
            } else if self.failed.borrow().is_none() {
                expecting
                    .said
                    .get(depth)
                    .filter(|said| !said.is_empty())
                    .cloned()
            } else {
                None
            }
        };
        self.settle(outcome, chain, fields, expected)
    }

    /// `outcome` as it is. The first failure is kept with where it happened —
    /// the innermost, as a failure travels outwards — and any success drops
    /// it, since a caller that succeeds recovered from it.
    fn settle<T, E>(
        &self,
        outcome: Result<T, E>,
        chain: &Chain<'_>,
        fields: Option<Names>,
        expected: Option<String>,
    ) -> Result<T, E> {
        let mut failed = self.failed.borrow_mut();
        match outcome {
            Ok(value) => {
                *failed = None;
                Ok(value)
            }
            Err(error) => {
                if failed.is_none() {
                    *failed = Some(Failed {
                        path: chain.path(),
                        fields,
                        expected,
                    });
                }
                Err(error)
            }
        }
    }
}

/// What a visitor reads keys against: the names a derive declared, if any.
#[derive(Clone, Copy)]
enum Declared {
    Nothing,
    Fields(Names),
    Variants(Names),
}

/// A deserializer that tracks the calls made on it and on what it hands on.
struct Tracked<'a, 'b, D> {
    de: D,
    chain: Chain<'a>,
    track: &'b Track,
}

macro_rules! tracked_calls {
    ($($method:ident($($arg:ident: $ty:ty),*))*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, D::Error> {
            let Self { de, chain, track } = self;
            track.enter(&visitor);
            let outcome = de.$method($($arg,)* Wrap::new(visitor, &chain, track, Declared::Nothing));
            track.leave(outcome, &chain, None)
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Tracked<'_, '_, D> {
    type Error = D::Error;

    tracked_calls! {
        deserialize_any() deserialize_bool()
        deserialize_i8() deserialize_i16() deserialize_i32() deserialize_i64() deserialize_i128()
        deserialize_u8() deserialize_u16() deserialize_u32() deserialize_u64() deserialize_u128()
        deserialize_f32() deserialize_f64() deserialize_char()
        deserialize_str() deserialize_string() deserialize_bytes() deserialize_byte_buf()
        deserialize_option() deserialize_unit()
        deserialize_unit_struct(name: &'static str)
        deserialize_newtype_struct(name: &'static str)
        deserialize_seq() deserialize_tuple(len: usize)
        deserialize_tuple_struct(name: &'static str, len: usize)
        deserialize_map() deserialize_identifier() deserialize_ignored_any()
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: Names,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        let Self { de, chain, track } = self;
        track.enter(&visitor);
        let outcome = de.deserialize_struct(
            name,
            fields,
            Wrap::new(visitor, &chain, track, Declared::Fields(fields)),
        );
        track.leave(outcome, &chain, Some(fields))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: Names,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        let Self { de, chain, track } = self;
        track.enter(&visitor);
        let outcome = de.deserialize_enum(
            name,
            variants,
            Wrap::new(visitor, &chain, track, Declared::Variants(variants)),
        );
        track.leave(outcome, &chain, None)
    }

    fn is_human_readable(&self) -> bool {
        self.de.is_human_readable()
    }
}

/// A visitor whose nested deserializers and accesses are tracked.
struct Wrap<'a, 'b, V> {
    delegate: V,
    chain: &'a Chain<'a>,
    track: &'b Track,
    declared: Declared,
}

impl<'a, 'b, V> Wrap<'a, 'b, V> {
    fn new(delegate: V, chain: &'a Chain<'a>, track: &'b Track, declared: Declared) -> Self {
        Self {
            delegate,
            chain,
            track,
            declared,
        }
    }
}

macro_rules! forwarded_visits {
    ($($method:ident($ty:ty))*) => {$(
        fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            self.delegate.$method(value)
        }
    )*};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Wrap<'_, '_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.delegate.expecting(f)
    }

    forwarded_visits! {
        visit_bool(bool) visit_i8(i8) visit_i16(i16) visit_i32(i32) visit_i64(i64) visit_i128(i128)
        visit_u8(u8) visit_u16(u16) visit_u32(u32) visit_u64(u64) visit_u128(u128)
        visit_f32(f32) visit_f64(f64) visit_char(char)
        visit_str(&str) visit_borrowed_str(&'de str) visit_string(String)
        visit_bytes(&[u8]) visit_borrowed_bytes(&'de [u8]) visit_byte_buf(Vec<u8>)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.delegate.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.delegate.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.delegate.visit_some(Tracked {
            de,
            chain: *self.chain,
            track: self.track,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.delegate.visit_newtype_struct(Tracked {
            de,
            chain: *self.chain,
            track: self.track,
        })
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        self.delegate.visit_seq(SeqAccess {
            delegate: seq,
            chain: self.chain,
            track: self.track,
            index: 0,
        })
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let fields = match self.declared {
            Declared::Fields(fields) => Some(fields),
            Declared::Nothing | Declared::Variants(_) => None,
        };
        self.delegate.visit_map(MapAccess {
            delegate: map,
            chain: self.chain,
            track: self.track,
            fields,
            key: None,
        })
    }

    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        let variants = match self.declared {
            Declared::Variants(variants) => Some(variants),
            Declared::Nothing | Declared::Fields(_) => None,
        };
        self.delegate.visit_enum(EnumAccess {
            delegate: data,
            chain: self.chain,
            track: self.track,
            variants,
        })
    }
}

/// A seed whose deserializer is tracked one step further down.
struct TrackedSeed<'a, 'b, S> {
    seed: S,
    chain: Chain<'a>,
    track: &'b Track,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for TrackedSeed<'_, '_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<S::Value, D::Error> {
        let Self { seed, chain, track } = self;
        let outcome = seed.deserialize(Tracked { de, chain, track });
        track.settle(outcome, &chain, None, None)
    }
}

/// A sequence whose elements are tracked by their index.
struct SeqAccess<'a, 'b, A> {
    delegate: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    index: usize,
}

impl<'de, A: de::SeqAccess<'de>> de::SeqAccess<'de> for SeqAccess<'_, '_, A> {
    type Error = A::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        let chain = Chain::Step {
            parent: self.chain,
            step: Step::Index(self.index),
        };
        self.index += 1;
        let track = self.track;
        let outcome = self
            .delegate
            .next_element_seed(TrackedSeed { seed, chain, track });
        track.settle(outcome, self.chain, None, None)
    }

    fn size_hint(&self) -> Option<usize> {
        self.delegate.size_hint()
    }
}

/// A map whose values are tracked by their key: a field its struct declares
/// by its name, any other key as an entry.
struct MapAccess<'a, 'b, A> {
    delegate: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    fields: Option<Names>,
    key: Option<Step>,
}

impl<'de, A: de::MapAccess<'de>> de::MapAccess<'de> for MapAccess<'_, '_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        let track = self.track;
        let outcome = match self.fields {
            Some(fields) => {
                let mut named = None;
                let outcome = self.delegate.next_key_seed(KeySeed {
                    seed,
                    names: fields,
                    named: &mut named,
                });
                self.key = Some(named.map_or(Step::Entry, Step::Field));
                outcome
            }
            None => {
                self.key = Some(Step::Entry);
                self.delegate.next_key_seed(seed)
            }
        };
        track.settle(outcome, self.chain, None, None)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        let chain = Chain::Step {
            parent: self.chain,
            step: self.key.take().unwrap_or(Step::Entry),
        };
        let track = self.track;
        let outcome = self
            .delegate
            .next_value_seed(TrackedSeed { seed, chain, track });
        track.settle(outcome, self.chain, None, None)
    }

    fn size_hint(&self) -> Option<usize> {
        self.delegate.size_hint()
    }
}

/// An enum whose content is tracked under the variant it names.
struct EnumAccess<'a, 'b, A> {
    delegate: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    variants: Option<Names>,
}

impl<'a, 'b, 'de, A: de::EnumAccess<'de>> de::EnumAccess<'de> for EnumAccess<'a, 'b, A> {
    type Error = A::Error;
    type Variant = VariantAccess<'a, 'b, A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), A::Error> {
        let Self {
            delegate,
            chain,
            track,
            variants,
        } = self;
        let mut named = None;
        let outcome = match variants {
            Some(variants) => delegate.variant_seed(KeySeed {
                seed,
                names: variants,
                named: &mut named,
            }),
            None => delegate.variant_seed(seed),
        };
        let step = named.map_or(Step::Entry, Step::Variant);
        track
            .settle(outcome, chain, None, None)
            .map(|(value, variant)| {
                let access = VariantAccess {
                    delegate: variant,
                    chain: Chain::Step {
                        parent: chain,
                        step,
                    },
                    track,
                };
                (value, access)
            })
    }
}

/// A variant's content, tracked under its name.
struct VariantAccess<'a, 'b, A> {
    delegate: A,
    chain: Chain<'a>,
    track: &'b Track,
}

impl<'de, A: de::VariantAccess<'de>> de::VariantAccess<'de> for VariantAccess<'_, '_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        let outcome = self.delegate.unit_variant();
        self.track.settle(outcome, &self.chain, None, None)
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, A::Error> {
        let Self {
            delegate,
            chain,
            track,
        } = self;
        let outcome = delegate.newtype_variant_seed(TrackedSeed { seed, chain, track });
        track.settle(outcome, &chain, None, None)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        let Self {
            delegate,
            chain,
            track,
        } = self;
        track.enter(&visitor);
        let outcome =
            delegate.tuple_variant(len, Wrap::new(visitor, &chain, track, Declared::Nothing));
        track.leave(outcome, &chain, None)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: Names,
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        let Self {
            delegate,
            chain,
            track,
        } = self;
        track.enter(&visitor);
        let outcome = delegate.struct_variant(
            fields,
            Wrap::new(visitor, &chain, track, Declared::Fields(fields)),
        );
        track.leave(outcome, &chain, Some(fields))
    }
}

/// A key's seed that learns which of `names` the key is, if any — the name
/// out of the list, never the key's text.
struct KeySeed<'n, S> {
    seed: S,
    names: Names,
    named: &'n mut Option<&'static str>,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for KeySeed<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<S::Value, D::Error> {
        let Self { seed, names, named } = self;
        seed.deserialize(KeyDeserializer { de, names, named })
    }
}

/// A key's deserializer, whose visitor learns the key.
struct KeyDeserializer<'n, D> {
    de: D,
    names: Names,
    named: &'n mut Option<&'static str>,
}

macro_rules! key_calls {
    ($($method:ident($($arg:ident: $ty:ty),*))*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, D::Error> {
            let Self { de, names, named } = self;
            de.$method($($arg,)* KeyVisitor { delegate: visitor, names, named })
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for KeyDeserializer<'_, D> {
    type Error = D::Error;

    key_calls! {
        deserialize_any() deserialize_bool()
        deserialize_i8() deserialize_i16() deserialize_i32() deserialize_i64() deserialize_i128()
        deserialize_u8() deserialize_u16() deserialize_u32() deserialize_u64() deserialize_u128()
        deserialize_f32() deserialize_f64() deserialize_char()
        deserialize_str() deserialize_string() deserialize_bytes() deserialize_byte_buf()
        deserialize_option() deserialize_unit()
        deserialize_unit_struct(name: &'static str)
        deserialize_newtype_struct(name: &'static str)
        deserialize_seq() deserialize_tuple(len: usize)
        deserialize_tuple_struct(name: &'static str, len: usize)
        deserialize_map()
        deserialize_struct(name: &'static str, fields: Names)
        deserialize_enum(name: &'static str, variants: Names)
        deserialize_identifier() deserialize_ignored_any()
    }

    fn is_human_readable(&self) -> bool {
        self.de.is_human_readable()
    }
}

/// A key's visitor, noting which declared name the key spells.
struct KeyVisitor<'n, V> {
    delegate: V,
    names: Names,
    named: &'n mut Option<&'static str>,
}

impl<V> KeyVisitor<'_, V> {
    fn learn(&mut self, key: &str) {
        *self.named = self.names.iter().copied().find(|name| *name == key);
    }
}

impl<'de, V: Visitor<'de>> Visitor<'de> for KeyVisitor<'_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.delegate.expecting(f)
    }

    forwarded_visits! {
        visit_bool(bool) visit_i8(i8) visit_i16(i16) visit_i32(i32) visit_i64(i64) visit_i128(i128)
        visit_u8(u8) visit_u16(u16) visit_u32(u32) visit_u64(u64) visit_u128(u128)
        visit_f32(f32) visit_f64(f64) visit_char(char) visit_byte_buf(Vec<u8>)
    }

    fn visit_str<E: de::Error>(mut self, value: &str) -> Result<Self::Value, E> {
        self.learn(value);
        self.delegate.visit_str(value)
    }

    fn visit_borrowed_str<E: de::Error>(mut self, value: &'de str) -> Result<Self::Value, E> {
        self.learn(value);
        self.delegate.visit_borrowed_str(value)
    }

    fn visit_string<E: de::Error>(mut self, value: String) -> Result<Self::Value, E> {
        self.learn(&value);
        self.delegate.visit_string(value)
    }

    fn visit_bytes<E: de::Error>(mut self, value: &[u8]) -> Result<Self::Value, E> {
        if let Ok(key) = std::str::from_utf8(value) {
            self.learn(key);
        }
        self.delegate.visit_bytes(value)
    }

    fn visit_borrowed_bytes<E: de::Error>(mut self, value: &'de [u8]) -> Result<Self::Value, E> {
        if let Ok(key) = std::str::from_utf8(value) {
            self.learn(key);
        }
        self.delegate.visit_borrowed_bytes(value)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.delegate.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.delegate.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.delegate.visit_some(de)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.delegate.visit_newtype_struct(de)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        self.delegate.visit_seq(seq)
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.delegate.visit_map(map)
    }

    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        self.delegate.visit_enum(data)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    fn failed<T: for<'de> Deserialize<'de> + fmt::Debug>(payload: &str) -> Failed {
        trace::<T, _>(&mut serde_json::Deserializer::from_str(payload))
            .expect_err("the payload does not decode")
            .1
    }

    fn path_of<T: for<'de> Deserialize<'de> + fmt::Debug>(payload: &str) -> String {
        failed::<T>(payload).path.to_string()
    }

    #[derive(Debug, Deserialize)]
    #[expect(
        dead_code,
        reason = "the fields exist for serde to read; the tests assert where it failed"
    )]
    struct Post {
        author: Author,
        tags: Vec<String>,
        limits: HashMap<String, u32>,
        payment: Option<Payment>,
    }

    #[derive(Debug, Deserialize)]
    #[expect(
        dead_code,
        reason = "the field exists for serde to read; the tests assert where it failed"
    )]
    struct Author {
        #[serde(rename = "fullName")]
        name: String,
    }

    #[derive(Debug, Deserialize)]
    #[expect(
        dead_code,
        reason = "the variant's field exists for serde to read; the tests assert where it failed"
    )]
    enum Payment {
        Card { number: u64 },
    }

    #[test]
    fn a_failure_is_placed_by_the_names_its_type_declares() {
        assert_eq!(
            path_of::<Post>(r#"{"author":{"fullName":1}}"#),
            "author.fullName"
        );
        assert_eq!(
            path_of::<Post>(r#"{"author":{"fullName":"a"},"tags":["x",1]}"#),
            "tags[1]"
        );
        assert_eq!(
            path_of::<Post>(
                r#"{"author":{"fullName":"a"},"tags":[],"payment":{"Card":{"number":"x"}}}"#
            ),
            "payment.Card.number"
        );
        assert_eq!(
            path_of::<Vec<Author>>(r#"[{"fullName":"a"},{"fullName":2}]"#),
            "[1].fullName"
        );
    }

    #[test]
    fn a_key_the_payload_chose_is_an_entry_never_its_text() {
        let path = path_of::<Post>(
            r#"{"author":{"fullName":"a"},"tags":[],"limits":{"sk_live_x":"seven"}}"#,
        );
        assert_eq!(path, "limits[…]");
        assert_eq!(
            path_of::<HashMap<String, u32>>(r#"{"sk_live_x":"seven"}"#),
            "[…]"
        );

        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            dead_code,
            reason = "the field exists for serde to read; the test asserts where it failed"
        )]
        struct Strict {
            amount: u64,
        }
        assert_eq!(path_of::<Strict>(r#"{"amount":1,"sk_live_x":1}"#), "");
    }

    #[test]
    fn a_struct_serde_reads_as_a_map_says_its_fields_as_entries() {
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts where it failed"
        )]
        struct Loose {
            count: u32,
            #[serde(flatten)]
            rest: HashMap<String, serde_json::Value>,
        }
        assert_eq!(path_of::<Loose>(r#"{"count":"x"}"#), "[…]");
    }

    #[test]
    fn a_struct_s_own_failure_carries_its_declared_fields() {
        let missing = failed::<Author>("{}");
        assert_eq!(missing.path.to_string(), "");
        assert_eq!(missing.fields, Some(&["fullName"][..]));
    }

    /// Refuses an odd number in its own words, quoting it.
    #[derive(Debug)]
    struct Even;

    impl<'de> Deserialize<'de> for Even {
        fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            struct Visit;
            impl Visitor<'_> for Visit {
                type Value = Even;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an even number")
                }
                fn visit_u64<E: de::Error>(self, value: u64) -> Result<Even, E> {
                    if value.is_multiple_of(2) {
                        Ok(Even)
                    } else {
                        Err(E::custom(format!("{value} is odd")))
                    }
                }
            }
            de.deserialize_u64(Visit)
        }
    }

    /// Accepts any string, then refuses it in its own words.
    #[derive(Debug)]
    struct Handle;

    impl<'de> Deserialize<'de> for Handle {
        fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            let handle = String::deserialize(de)?;
            Err(de::Error::custom(format!("{handle} is taken")))
        }
    }

    #[test]
    fn what_was_expected_is_the_refusing_visitor_s_alone() {
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts where it failed"
        )]
        struct Signup {
            lucky: Even,
            handle: Handle,
        }
        let odd = failed::<Signup>(r#"{"lucky":7,"handle":"x"}"#);
        assert_eq!(odd.path.to_string(), "lucky");
        assert_eq!(odd.expected.as_deref(), Some("an even number"));

        let taken = failed::<Signup>(r#"{"lucky":8,"handle":"sk_live_x"}"#);
        assert_eq!(taken.path.to_string(), "handle");
        assert_eq!(taken.expected, None, "the string it read was a string");
    }

    #[test]
    fn a_refusal_after_a_read_that_succeeded_is_not_its_holder_s_expectation() {
        #[derive(Debug, Deserialize)]
        #[serde(try_from = "String")]
        struct Email;
        impl TryFrom<String> for Email {
            type Error = String;
            fn try_from(text: String) -> Result<Self, String> {
                Err(format!("{text} is no address"))
            }
        }
        #[derive(Debug, Deserialize)]
        struct Wrapper(Email);
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts where it failed"
        )]
        struct Invite {
            email: Option<Email>,
            backup: Option<Wrapper>,
        }

        let optional = failed::<Invite>(r#"{"email":"sk_live_51"}"#);
        assert_eq!(optional.path.to_string(), "email");
        assert_eq!(optional.expected, None, "the option read its string");

        let wrapped = failed::<Invite>(r#"{"email":null,"backup":"sk_live_51"}"#);
        assert_eq!(wrapped.path.to_string(), "backup");
        assert_eq!(wrapped.expected, None, "the newtype read its string");
    }

    #[test]
    fn a_failure_a_caller_recovered_from_is_forgotten() {
        /// Reads a number when there is one, and nothing otherwise.
        #[derive(Debug)]
        struct Lenient;
        impl<'de> Deserialize<'de> for Lenient {
            fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                drop(u32::deserialize(de));
                Ok(Lenient)
            }
        }
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts where it failed"
        )]
        struct Counted {
            lenient: Lenient,
            count: u32,
        }
        let payload = json!({"lenient": "x", "count": "y"});
        let refused = trace::<Counted, _>(&payload)
            .expect_err("count is no number")
            .1;
        assert_eq!(refused.path.to_string(), "count");
    }

    #[test]
    fn a_path_renders_fields_with_dots_and_positions_in_brackets() {
        let path = DecodePath(vec![
            Step::Field("items"),
            Step::Index(2),
            Step::Field("tags"),
            Step::Entry,
            Step::Variant("Card"),
        ]);
        assert_eq!(path.to_string(), "items[2].tags[…].Card");
        assert_eq!(
            DecodePath(vec![Step::Entry, Step::Field("a")]).to_string(),
            "[…].a"
        );
    }
}
