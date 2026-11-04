//! A field error's keys, as the schema spells them.
//!
//! A field map names a field by its Rust name, validation's and a `#[crud]`
//! conflict's alike, where a client sent the name the schema serves, so it could
//! not match the one to the other. The rename rule is async-graphql's derive's,
//! and all it leaves at run time is its result, the names in the registry: a key
//! is matched to those, never re-cased, so an input under any rule answers in the
//! spelling it declares.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_graphql::registry::{MetaType, Registry};
use async_graphql::{Context, Error, Name, Value};

use crate::error::FIELD_ERRORS_EXTENSION;

/// Every spelling a schema's inputs and arguments serve, by the word it spells.
type Index = HashMap<String, Vec<String>>;

/// A schema's [`Index`], read off its registry by the first field error that needs
/// it: a request that fails on nothing pays for none of it.
#[derive(Default)]
pub(crate) struct FieldSpellings(OnceLock<Index>);

/// `error`, the keys of its field map spelled as `ctx`'s schema spells them.
pub(crate) fn spelled_by_schema(ctx: &Context<'_>, mut error: Error) -> Error {
    if let Some(extensions) = error.extensions.as_mut()
        && let Some(fields) = extensions.get(FIELD_ERRORS_EXTENSION)
        && let Some(spellings) = ctx.data_opt::<FieldSpellings>()
    {
        let index = spellings
            .0
            .get_or_init(|| index_of(&ctx.schema_env.registry));
        let spelled = respelled(fields, index);
        extensions.set(FIELD_ERRORS_EXTENSION, spelled);
    }
    error
}

fn index_of(registry: &Registry) -> Index {
    let mut index = Index::new();
    let mut add = |name: &String| {
        let spellings = index.entry(word_of(name)).or_default();
        if !spellings.contains(name) {
            spellings.push(name.clone());
        }
    };
    for ty in registry.types.values() {
        match ty {
            MetaType::InputObject { input_fields, .. } => input_fields.keys().for_each(&mut add),
            MetaType::Object { fields, .. } | MetaType::Interface { fields, .. } => fields
                .values()
                .flat_map(|field| field.args.keys())
                .for_each(&mut add),
            MetaType::Scalar { .. } | MetaType::Enum { .. } | MetaType::Union { .. } => {}
        }
    }
    index
}

/// What a rename rule leaves of a name: async-graphql's rules move only its case
/// and its underscores.
fn word_of(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A field map with its keys spelled through `index`, at every depth: a nested
/// input's failures are a map under its field. An array is a field's failures,
/// whose members name no field.
fn respelled(fields: &Value, index: &Index) -> Value {
    match fields {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (spelled(key, index), respelled(value, index)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `key` as the schema spells it, or as it was said when no input spells it. A key
/// one of the schema's own spellings stays, since inputs under two rules can both
/// serve its word. GraphQL reserves `__`, so `__all__`, validator's key for a rule
/// over a whole struct, is no field whatever its word.
fn spelled(key: &Name, index: &Index) -> Name {
    if key.starts_with("__") {
        return key.clone();
    }
    index
        .get(&word_of(key))
        .filter(|spellings| !spellings.iter().any(|spelling| spelling == key.as_str()))
        .and_then(|spellings| spellings.first())
        .map_or_else(|| key.clone(), Name::new)
}

#[cfg(test)]
mod tests {
    use async_graphql::{InputObject, InputType};

    use super::*;

    #[derive(InputObject)]
    struct CamelInput {
        org_id: String,
        get_2fa: String,
    }

    #[derive(InputObject)]
    #[graphql(rename_fields = "snake_case")]
    struct SnakeInput {
        org_id: String,
        tax_code: String,
    }

    /// What a schema declaring `inputs` serves.
    fn index(inputs: &[fn(&mut Registry) -> String]) -> Index {
        let mut registry = Registry::default();
        for register in inputs {
            register(&mut registry);
        }
        index_of(&registry)
    }

    fn field_map(json: serde_json::Value) -> Value {
        Value::from_json(json).expect("plain JSON converts")
    }

    #[test]
    fn a_key_takes_the_spelling_its_input_declares_at_every_depth() {
        let answered = respelled(
            &field_map(serde_json::json!({
                "org_id": [{ "code": "length" }],
                "get_2fa": [{ "code": "length", "org_id": "not a key" }],
                "nested": { "org_id": { "0": { "org_id": [] } } },
            })),
            &index(&[CamelInput::create_type_info]),
        );

        assert_eq!(
            answered.into_json().expect("converts back"),
            serde_json::json!({
                "orgId": [{ "code": "length" }],
                "get2Fa": [{ "code": "length", "org_id": "not a key" }],
                "nested": { "orgId": { "0": { "orgId": [] } } },
            }),
        );
    }

    #[test]
    fn a_key_no_input_serves_stays_as_it_was_said() {
        assert_eq!(
            spelled(
                &Name::new("tax_code"),
                &index(&[CamelInput::create_type_info])
            )
            .as_str(),
            "tax_code"
        );
    }

    #[test]
    fn validators_key_for_a_whole_struct_is_no_field() {
        #[derive(InputObject)]
        struct WithAll {
            all: String,
        }

        assert_eq!(
            spelled(&Name::new("__all__"), &index(&[WithAll::create_type_info])).as_str(),
            "__all__"
        );
    }

    #[test]
    fn a_key_one_of_two_rules_spells_stays_where_a_schema_mixes_them() {
        let index = index(&[CamelInput::create_type_info, SnakeInput::create_type_info]);

        assert_eq!(spelled(&Name::new("org_id"), &index).as_str(), "org_id");
        assert_eq!(spelled(&Name::new("tax_code"), &index).as_str(), "tax_code");
    }
}
