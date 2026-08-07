//! Template rendering over a `{{key}}` variable map.
//!
//! Replaces the old hand-maintained `render_with_extra` whose keys were
//! hard-coded. A `Renderer` seeds every identifier derived from [`Names`]
//! and lets a generator layer extra vars (`port`, adapter flags) on top.

use std::collections::HashMap;

use crate::naming::{Names, Transport};

#[derive(Clone)]
pub struct Renderer {
    vars: HashMap<String, String>,
}

impl Renderer {
    /// Seed the standard identifiers for `names`. Every key below is
    /// available as `{{key}}` in any template string.
    pub fn new(names: &Names) -> Self {
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
        put("queue_name", names.queue_name());
        put("tasks", names.tasks());
        put("tool", names.tool());
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
        // The `nest-rs-*` version every generated manifest pins — derived from
        // the CLI's own version so it can never go stale (see `crate::version`).
        put("nestrs_version", crate::version::framework_req());
        // The span-target root the conventions show an example under: the app's
        // own name, which is what a single crate emits. A workspace overrides it
        // to the shared feature library's root. Seeded here rather than only at
        // that override, for the same reason as the two prefix keys below.
        put("span_target", format!("{}::users", names.snake));
        // Every env-var name a template writes goes through these keys. The
        // framework default stands unless a caller that knows the project
        // (`nestrs new --env-prefix`) overrides it — a template must never spell
        // `NESTRS_` itself, which
        // `templates::tests::templates_use_the_env_prefix_placeholder_not_a_literal`
        // enforces.
        put("env_prefix", crate::context::DEFAULT_ENV_PREFIX.to_owned());
        put("env_prefix_var", crate::context::ENV_PREFIX_VAR.to_owned());
        // The two lines that *set* the prefix on the processes a project starts
        // — empty unless overridden, because a project on the default sets
        // nothing. Seeded here rather than only in `with_env_prefix`: a renderer
        // that forgot the override would otherwise write the placeholder itself
        // into a Justfile, which no compiler would ever notice.
        put("env_prefix_export", String::new());
        put("env_prefix_env", String::new());
        Self { vars }
    }

    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.vars.insert(key.to_string(), value.into());
        self
    }

    pub fn render(&self, template: &str) -> String {
        let mut out = template.to_string();
        for (key, value) in &self.vars {
            out = out.replace(&format!("{{{{{key}}}}}"), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_templates_use_the_version_placeholder_not_a_literal() {
        // Version-independent: the raw template must defer to the placeholder
        // so it can never freeze at a literal that rots on the next release.
        for cargo in [
            crate::templates::standalone::CARGO,
            crate::templates::workspace::ROOT_CARGO,
        ] {
            assert!(
                cargo.contains("version = \"{{nestrs_version}}\""),
                "nest-rs pins must use the {{nestrs_version}} placeholder"
            );
            assert!(
                !cargo.contains("nest-rs = { version = \"0."),
                "a hard-coded nest-rs version would rot on release"
            );
        }
    }

    #[test]
    fn renderer_substitutes_the_derived_framework_version() {
        let r = Renderer::new(&Names::parse("demo"));
        let rendered = r.render(crate::templates::standalone::CARGO);
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
        // The scaffolded queue payload is a verb-led Command.
        assert_eq!(r.render("{{command}}"), "ProcessPostCommand");
    }
}
