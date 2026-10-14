//! `#[instance]`'s grammar and name: a marker's name is its ident in snake
//! case, read and never chosen, and refused where a namespace could not carry it.

use syn::Ident;
use syn::ext::IdentExt;

use crate::Grammar;
use crate::casing::snake_case;

/// `#[instance]` takes no argument: a key someone writes is told where the name
/// comes from.
pub const INSTANCE: Grammar = Grammar::new("instance", &[]).elsewhere(named_by_its_ident);

/// The default instance's marker, which no `#[instance]` may take.
pub const DEFAULT_INSTANCE: &str = "Unnamed";

/// The layout's structural vocabulary — `architecture.md`'s *Reserved
/// vocabulary* — which no instance may take: a namespace segment there would
/// read as a binding's or a folder's.
pub const RESERVED_VOCABULARY: &[&str] = &[
    "apps",
    "crates",
    "features",
    "src",
    "tests",
    "mod",
    "module",
    "service",
    "controller",
    "resolver",
    "gateway",
    "tool",
    "processor",
    "tasks",
    "listener",
    "guard",
    "strategy",
    "pipe",
    "config",
    "interceptor",
    "filter",
    "entity",
    "error",
    "constants",
    "testing",
    "dto",
    "command",
    "event",
    "services",
    "entities",
    "dtos",
    "commands",
    "strategies",
    "pipes",
    "http",
    "graphql",
    "ws",
    "queue",
    "schedule",
    "mcp",
    "events",
];

fn named_by_its_ident(key: &str) -> Option<String> {
    Some(format!(
        "#[instance] takes no arguments, and `{key}` is none: an instance's name is its \
         struct's ident in snake case, read and never chosen — rename the struct"
    ))
}

/// The name the marker `ident` reads — `ReadReplica` reads `read_replica` — or
/// the sentence refusing it.
pub fn instance_name(ident: &Ident) -> Result<String, String> {
    let written = ident.unraw().to_string();
    if written == DEFAULT_INSTANCE {
        return Err(format!(
            "#[instance]: `{DEFAULT_INSTANCE}` is the default instance's marker, which adds no \
             segment to a namespace — name this one for what it holds, e.g. `Cache`"
        ));
    }
    if !written
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!(
            "#[instance]: `{written}` holds a character an environment variable cannot carry — \
             an instance's name is its ident in snake case, inserted in its config's variables, \
             so it is ASCII letters, digits and `_`"
        ));
    }
    let name = snake_case(&written);
    if name.contains("__") || name.starts_with('_') || name.ends_with('_') {
        return Err(format!(
            "#[instance]: `{written}` reads `{name}`, and `__` separates a namespace's segments — \
             an instance's name holds no `__` and neither starts nor ends with `_`"
        ));
    }
    if RESERVED_VOCABULARY.contains(&name.as_str()) {
        return Err(format!(
            "#[instance]: `{written}` reads `{name}`, a word the layout reserves — as a \
             namespace segment it would read as a binding's or a folder's; name the instance \
             for what it holds"
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn a_name_is_the_ident_in_snake_case() {
        for (ident, name) in [
            (parse_quote!(Cache), "cache"),
            (parse_quote!(ReadReplica), "read_replica"),
            (parse_quote!(r#Lock), "lock"),
            (parse_quote!(EU2), "eu2"),
        ] {
            assert_eq!(instance_name(&ident).as_deref(), Ok(name));
        }
    }

    #[test]
    fn a_name_a_namespace_cannot_carry_is_refused_saying_why() {
        let ident: Ident = parse_quote!(Unnamed);
        assert!(
            instance_name(&ident)
                .unwrap_err()
                .contains("the default instance's marker")
        );
        for ident in [parse_quote!(Read_Replica), parse_quote!(_Cache)] {
            let refused = instance_name(&ident).unwrap_err();
            assert!(refused.contains("holds no `__`"), "{refused}");
        }
        let ident: Ident = parse_quote!(Café);
        assert!(
            instance_name(&ident)
                .unwrap_err()
                .contains("ASCII letters, digits and `_`")
        );
        for ident in [
            parse_quote!(Queue),
            parse_quote!(Http),
            parse_quote!(Service),
        ] {
            let refused = instance_name(&ident).unwrap_err();
            assert!(refused.contains("a word the layout reserves"), "{refused}");
        }
    }
}
