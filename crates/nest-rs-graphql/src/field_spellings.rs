//! A field error's keys, as the schema spells them.
//!
//! A field map names a field by its Rust name, validation's and a `#[crud]`
//! conflict's alike, where a client sent the name the schema serves, so it could
//! not match the one to the other. The rename rule is async-graphql's derive's,
//! and all it leaves at run time is its result, the names in the registry: a key
//! is matched to those, never re-cased, so an input under any rule answers in the
//! spelling it declares. A column a refusal names without any selection naming it,
//! a primary key a cursor reads, is matched to the objects' names the same way.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_graphql::registry::{MetaType, Registry};
use async_graphql::{Context, Error, Name, Value};

use crate::error::FIELD_ERRORS_EXTENSION;

/// Every spelling a schema's inputs and arguments serve, by the word it spells.
type Index = HashMap<String, Vec<String>>;

/// A schema's [`Index`]es, each read off its registry by the first refusal that
/// needs it: a request that fails on nothing pays for none of it.
#[derive(Default)]
pub(crate) struct FieldSpellings {
    /// What its inputs and arguments serve.
    inputs: OnceLock<Index>,
    /// What its objects and interfaces answer.
    outputs: OnceLock<Index>,
}

/// `error`, the keys of its field map spelled as `ctx`'s schema spells them.
pub(crate) fn spelled_by_schema(ctx: &Context<'_>, mut error: Error) -> Error {
    if let Some(extensions) = error.extensions.as_mut()
        && let Some(fields) = extensions.get(FIELD_ERRORS_EXTENSION)
        && let Some(spellings) = ctx.data_opt::<FieldSpellings>()
    {
        let index = spellings
            .inputs
            .get_or_init(|| inputs_of(&ctx.schema_env.registry));
        let spelled = respelled(fields, index);
        extensions.set(FIELD_ERRORS_EXTENSION, spelled);
    }
    error
}

/// `key`, a column's wire key, as `ctx`'s schema spells that field on its objects, or
/// as it was said when none serves it.
pub fn schema_field_spelling<'c>(ctx: &'c Context<'_>, key: &'c str) -> &'c str {
    let Some(spellings) = ctx.data_opt::<FieldSpellings>() else {
        return key;
    };
    let index = spellings
        .outputs
        .get_or_init(|| outputs_of(&ctx.schema_env.registry));
    spelling(key, index).unwrap_or(key)
}

/// `names` by the word each spells.
fn index_of<'r>(names: impl Iterator<Item = &'r String>) -> Index {
    let mut index = Index::new();
    for name in names {
        let spellings = index.entry(word_of(name)).or_default();
        if !spellings.contains(name) {
            spellings.push(name.clone());
        }
    }
    index
}

fn inputs_of(registry: &Registry) -> Index {
    index_of(registry.types.values().flat_map(|ty| -> Vec<&String> {
        match ty {
            MetaType::InputObject { input_fields, .. } => input_fields.keys().collect(),
            MetaType::Object { fields, .. } | MetaType::Interface { fields, .. } => fields
                .values()
                .flat_map(|field| field.args.keys())
                .collect(),
            MetaType::Scalar { .. } | MetaType::Enum { .. } | MetaType::Union { .. } => Vec::new(),
        }
    }))
}

fn outputs_of(registry: &Registry) -> Index {
    index_of(registry.types.values().flat_map(|ty| -> Vec<&String> {
        match ty {
            MetaType::Object { fields, .. } | MetaType::Interface { fields, .. } => {
                fields.keys().collect()
            }
            _ => Vec::new(),
        }
    }))
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

/// `key` as the schema spells it, or as it was said when no input spells it.
/// GraphQL reserves `__`, so `__all__`, validator's key for a rule over a whole
/// struct, is no field whatever its word.
fn spelled(key: &Name, index: &Index) -> Name {
    if key.starts_with("__") {
        return key.clone();
    }
    spelling(key, index).map_or_else(|| key.clone(), Name::new)
}

/// How the schema spells `key`, when it serves its word under another spelling. A key
/// one of the schema's own spellings stays, since names under two rules can both
/// serve its word.
fn spelling<'i>(key: &str, index: &'i Index) -> Option<&'i str> {
    index
        .get(&word_of(key))
        .filter(|spellings| !spellings.iter().any(|spelling| spelling == key))
        .and_then(|spellings| spellings.first())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use async_graphql::{InputObject, InputType, OutputType, SimpleObject};

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
        inputs_of(&registry)
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
    fn a_column_takes_the_spelling_its_object_declares_and_no_input_says_it() {
        #[derive(SimpleObject)]
        struct Camel {
            volume_id: String,
        }

        #[derive(SimpleObject)]
        #[graphql(rename_fields = "snake_case")]
        struct Snake {
            tax_code: String,
        }

        let mut registry = Registry::default();
        Camel::create_type_info(&mut registry);
        Snake::create_type_info(&mut registry);
        CamelInput::create_type_info(&mut registry);
        let outputs = outputs_of(&registry);

        assert_eq!(spelling("volume_id", &outputs), Some("volumeId"));
        assert_eq!(spelling("tax_code", &outputs), None, "its own spelling");
        assert_eq!(spelling("org_id", &outputs), None, "an input's alone");
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
