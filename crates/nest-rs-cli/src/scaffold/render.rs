//! Template rendering over a `{{key}}` variable map.
//!
//! Replaces the old hand-maintained `render_with_extra` whose keys were
//! hard-coded. A `Renderer` seeds every identifier derived from [`Names`]
//! and lets a generator layer extra vars (`port`, adapter flags) on top.

use std::collections::HashMap;

use crate::naming::{Names, Transport};

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
        put("tasks", names.tasks());
        put("tool", names.tool());
        put("entity", names.entity());
        put("table", names.table());
        put("create_input", names.create_input());
        put("update_input", names.update_input());
        put("http_module", names.module_for(Transport::Http));
        put("graphql_module", names.module_for(Transport::Graphql));
        put("ws_module", names.module_for(Transport::Ws));
        put("queue_module", names.module_for(Transport::Queue));
        put("schedule_module", names.module_for(Transport::Schedule));
        put("mcp_module", names.module_for(Transport::Mcp));
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
    fn renders_seeded_and_extra_vars() {
        let names = Names::parse("posts");
        let r = Renderer::new(&names).with("port", "3001");
        assert_eq!(
            r.render("{{module}} on {{port}} → {{entity}}"),
            "PostsModule on 3001 → Post"
        );
        assert_eq!(r.render("{{http_module}}"), "PostsHttpModule");
    }
}
