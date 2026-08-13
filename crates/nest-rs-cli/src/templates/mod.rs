use crate::naming::Transport;

pub mod adapter;
pub mod auth;
pub mod entity;
pub mod feature;
pub mod hello;
pub mod migration;
pub mod resource;
pub mod shared;
pub mod standalone;
pub mod workspace;

/// The handler a transport adapter renders, as `{{op}}` / `{{op_body}}` /
/// `{{op_value}}` / `{{op_description}}`.
///
/// One template per transport, two bodies. Over a `g feature` port the handler
/// delegates to the service's `count()`, showing the seam. Over a `g resource`
/// port it cannot — a `CrudService` has no `count()`, and its rows are only
/// reachable behind an ambient ability a skeleton must not fabricate — so the
/// body is a placeholder naming the call to write. Only the handler differs
/// between the two, so only the handler is a variable: the imports, the
/// `#[gateway]`/`#[tool_router]` scaffolding and the `/ws/<feature>` path
/// rationale keep one home each.
pub fn crud_vars(crud_port: bool, transport: Transport) -> Vec<(&'static str, String)> {
    if !crud_port {
        return vec![
            ("op", "count".to_owned()),
            ("op_body", String::new()),
            // An MCP tool answers with an owned `String`, so it renders the
            // conversion rather than a borrow the generated project's own
            // `clippy -D warnings` would reject.
            (
                "op_value",
                match transport {
                    Transport::Mcp => "self.svc.count().to_string()",
                    _ => "&self.svc.count()",
                }
                .to_owned(),
            ),
            ("op_description", "Count {{kebab}} items.".to_owned()),
        ];
    }

    let body = match transport {
        Transport::Ws => concat!(
            "        // SECURITY: rows are ability-scoped, and this gateway is unguarded as\n",
            "        // scaffolded — with no ambient `Ability` every `Repo` read denies all\n",
            "        // rows. Bind #[use_guards(AuthnGuard, AuthzGuard)] on the struct and\n",
            "        // import AuthzWsModule in this adapter's module.rs (it brings\n",
            "        // `WsDataContext`, which installs the executor + ability per message),\n",
            "        // then replace the placeholder with `self.svc.list().await`.\n",
            "        let _ = &self.svc;",
        ),
        Transport::Schedule => concat!(
            "        // A scheduled tick is system work: `DatabaseModule`'s JobContext\n",
            "        // installs the job executor with no ability, so `Repo` runs unscoped.\n",
            "        // Replace the placeholder with the call this tick should make, e.g.\n",
            "        // `self.svc.list().await?`.\n",
            "        let _ = &self.svc;",
        ),
        Transport::Mcp => concat!(
            "        // SECURITY: rows are ability-scoped. Bind the app's McpAbilityBridge as\n",
            "        // `dyn McpOperationGuard` and import AuthzMcpModule (it brings\n",
            "        // `McpDataContext`, which installs the executor + ability per tool call),\n",
            "        // then replace the placeholder with `self.svc.list().await` and mask the\n",
            "        // rows through `nest_rs::authz::masked_output_ambient`.\n",
            "        let _ = &self.svc;",
        ),
        // These three render no `{{op_body}}` over a resource: HTTP and GraphQL
        // take a template of their own (`resource::HTTP_CONTROLLER` /
        // `GRAPHQL_RESOLVER_CRUD`), and the queue processor is driven by its
        // payload type rather than by a read. Spelled out so a transport added
        // later cannot inherit another one's security note by falling into `_`.
        Transport::Http | Transport::Graphql | Transport::Queue => "",
    };
    let value = match transport {
        Transport::Ws => "&Vec::<String>::new()",
        Transport::Mcp => "\"[]\".to_owned()",
        _ => "\"[]\".to_string()",
    };
    vec![
        ("op", "list".to_owned()),
        ("op_body", body.to_owned()),
        ("op_value", value.to_owned()),
        ("op_description", "List {{kebab}} items.".to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    /// Every template module, as source. Scanned rather than enumerated by
    /// constant so a log added to a *new* template is covered without anyone
    /// remembering to extend a list.
    const SOURCES: &[&str] = &[
        include_str!("adapter.rs"),
        include_str!("auth.rs"),
        include_str!("entity.rs"),
        include_str!("feature.rs"),
        include_str!("hello.rs"),
        include_str!("migration.rs"),
        include_str!("resource.rs"),
        include_str!("shared.rs"),
        include_str!("standalone.rs"),
        include_str!("workspace.rs"),
    ];

    /// A template that spells `NESTRS_` writes a variable an `--env-prefix`
    /// project never reads — a `.env` key silently inert, or a generated tool
    /// looking at the wrong name. The placeholder is the only legal form, so
    /// the guard is mechanical rather than a review habit.
    #[test]
    fn templates_use_the_env_prefix_placeholder_not_a_literal() {
        let literals: Vec<&str> = SOURCES
            .iter()
            .flat_map(|src| src.lines())
            // Rust doc/line comments in the CLI's own source describe the
            // scheme; only the emitted template strings are the contract.
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains("NESTRS_"))
            .map(str::trim)
            .collect();
        assert!(
            literals.is_empty(),
            "templates must write {{{{env_prefix}}}}_, not a literal: {literals:#?}",
        );
    }

    /// `CLAUDE.md`: *metadata is mandatory — a bare log is a defect*. A scaffold
    /// emits what the rules mandate, so a template that logs without a field
    /// ships that defect into every generated project. The whole framework holds
    /// this at zero; the generated code has to as well.
    ///
    /// Matches the macro-call shape (`tracing::<level>!(target: "…"`), so prose
    /// mentioning `tracing::` never trips it.
    #[test]
    fn no_scaffolded_log_is_emitted_without_a_structured_field() {
        let bare: Vec<&str> = SOURCES
            .iter()
            .flat_map(|src| src.lines())
            .filter(|line| line.contains("tracing::") && line.contains("!(target: \""))
            .filter(|line| {
                // Between the target's closing quote and the message's opening
                // one there must be at least one `field = value` pair.
                line.split_once("!(target: \"")
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .is_some_and(|(_, after)| !after.split('"').next().unwrap_or("").contains('='))
            })
            .map(|line| line.trim())
            .collect();

        assert!(
            bare.is_empty(),
            "these scaffolded logs carry no structured field:\n{}",
            bare.join("\n"),
        );
    }
}
