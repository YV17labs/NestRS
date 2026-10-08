//! Template rendering over a `{{key}}` variable map.
//!
//! A `Renderer` seeds every identifier derived from [`Names`] and lets a
//! generator layer extra vars (`port`, adapter flags) on top.

use std::collections::HashMap;

use crate::naming::{Names, Transport};

#[derive(Clone)]
pub(crate) struct Renderer {
    vars: HashMap<String, String>,
}

impl Renderer {
    /// Seed the standard identifiers for `names`. Every key below is
    /// available as `{{key}}` in any template string.
    pub(crate) fn new(names: &Names) -> Self {
        let mut vars = HashMap::new();
        let mut put = |k: &str, v: String| {
            vars.insert(k.to_string(), v);
        };
        put("kebab", names.kebab.clone());
        put("snake", names.snake.clone());
        put("pascal", names.pascal.clone());
        put("singular", names.singular.clone());
        put("module", names.module());
        put("service", names.service());
        put("controller", names.controller());
        put("resolver", names.resolver());
        put("gateway", names.gateway());
        put("processor", names.processor());
        put("queue", names.queue());
        put("tasks", names.tasks());
        put("tool", names.tool());
        put("listener", names.listener());
        put("event", names.event());
        put("entity", names.entity());
        put("table", names.table());
        put("create_op", names.create_op());
        put("update_op", names.update_op());
        put("command", names.command());
        put("http_module", names.module_for(Transport::Http));
        put("graphql_module", names.module_for(Transport::Graphql));
        put("ws_module", names.module_for(Transport::Ws));
        put("schedule_module", names.module_for(Transport::Schedule));
        put("mcp_module", names.module_for(Transport::Mcp));
        // Derived from the CLI's own version (see `crate::version`).
        put("nestrs_version", crate::version::framework_req());
        // Every env-var name a template writes goes through these keys; a template
        // never spells `NESTRS_` itself.
        put("env_prefix", crate::context::DEFAULT_ENV_PREFIX.to_owned());
        put("env_prefix_var", crate::context::ENV_PREFIX_VAR.to_owned());
        // Seeded from the same list `with_env_prefix` re-seeds from.
        for (key, value) in crate::commands::prefix_vars(crate::context::DEFAULT_ENV_PREFIX) {
            put(key, value);
        }
        Self { vars }
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.vars.insert(key.to_string(), value.into());
        self
    }

    /// Substitute `{{key}}` for every key this renderer holds, repeatedly until
    /// nothing changes.
    ///
    /// A seeded *value* may itself contain a placeholder (`op_description` is
    /// `"Count {{kebab}} items."`) and `vars` is a `HashMap` iterated in random
    /// order, so one pass would leave `{{kebab}}` behind about half the time.
    /// Unknown placeholders are untouched, which lets a Justfile keep Just's
    /// `{{app}}`. Bounded by the key count.
    pub(crate) fn render(&self, template: &str) -> String {
        let mut out = template.to_string();
        for _ in 0..=self.vars.len() {
            let mut next = out.clone();
            for (key, value) in &self.vars {
                next = next.replace(&format!("{{{{{key}}}}}"), value);
            }
            if next == out {
                return out;
            }
            out = next;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seeded value carrying a placeholder resolves too, whatever order the
    /// `HashMap` yields.
    #[test]
    fn a_seeded_value_carrying_a_placeholder_is_resolved_too() {
        let r = Renderer::new(&crate::naming::Names::parse("widget"))
            .with("op_description", "Count {{kebab}} items.");
        assert_eq!(
            r.render("#[tool(description = \"{{op_description}}\")]"),
            "#[tool(description = \"Count widget items.\")]",
        );
    }

    /// …and a placeholder this renderer does not own survives untouched.
    #[test]
    fn a_placeholder_the_renderer_does_not_own_is_left_alone() {
        let r = Renderer::new(&crate::naming::Names::parse("widget"));
        assert_eq!(
            r.render("cargo run --bin {{app}} # {{kebab}}"),
            "cargo run --bin {{app}} # widget",
        );
    }

    #[test]
    fn cargo_templates_use_the_version_placeholder_not_a_literal() {
        let cargo = crate::templates::workspace::ROOT_CARGO;
        assert!(
            cargo.contains("version = \"{{nestrs_version}}\""),
            "nest-rs pins must use the {{nestrs_version}} placeholder"
        );
        assert!(
            !cargo.contains("nest-rs = { version = \"0."),
            "a hard-coded nest-rs version would rot on release"
        );
    }

    #[test]
    fn renderer_substitutes_the_derived_framework_version() {
        let r = Renderer::new(&Names::parse("demo"));
        let rendered = r.render(crate::templates::workspace::ROOT_CARGO);
        assert!(rendered.contains(&format!(
            "version = \"{}\"",
            crate::version::framework_req()
        )));
        assert!(!rendered.contains("{{nestrs_version}}"));
    }

    #[test]
    fn renders_seeded_and_extra_vars() {
        let names = Names::parse("posts");
        let r = Renderer::new(&names).with("port", "3001");
        assert_eq!(
            r.render("{{module}} on {{port}} → {{entity}}"),
            "PostsModule on 3001 → Post"
        );
        assert_eq!(r.render("{{http_module}}"), "PostsHttpModule");
        assert_eq!(r.render("{{command}}"), "ProcessPostCommand");
    }
}
