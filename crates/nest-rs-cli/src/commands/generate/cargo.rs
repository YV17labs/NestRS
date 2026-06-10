//! Dependency auto-wiring for generated code.
//!
//! Adding a transport adapter to a fresh workspace usually needs a crate the
//! starter `Cargo.toml` doesn't carry yet (a resource needs `nest-rs-seaorm`,
//! a GraphQL adapter needs `async-graphql`, …). These [`Transform`]s splice
//! the missing entries into the root `[workspace.dependencies]` and the
//! `crates/features` manifest — idempotently, so an already-equipped workspace
//! (the nestrs repo itself) is a no-op.

use toml_edit::{DocumentMut, Item, Value};

use crate::naming::Transport;
use crate::scaffold::Transform;
use crate::version::framework_req;

/// One dependency the generator may need to introduce.
pub(crate) struct Dep {
    name: &'static str,
    /// TOML value placed in `[workspace.dependencies]` when absent. **Ignored
    /// for `nest-rs-*` crates** — their version tracks the CLI's own release
    /// line (see [`framework_req`]), so leave it `""` for those.
    workspace_value: &'static str,
    /// Features to enable in the `features` crate (`[]` ⇒ `{ workspace = true }`).
    features: &'static [&'static str],
}

impl Dep {
    /// The `[workspace.dependencies]` value to insert. `nest-rs-*` crates pin
    /// the lockstep framework requirement; everything else uses its literal.
    fn workspace_item(&self) -> Item {
        if self.name.starts_with("nest-rs-") {
            parse_value(&format!("\"{}\"", framework_req()))
        } else {
            parse_value(self.workspace_value)
        }
    }
}

// `nest-rs-*` crates: `workspace_value` is unused — `workspace_item` derives
// the version from the CLI's own release line (`framework_req`).
const SEAORM: Dep = Dep {
    name: "nest-rs-seaorm",
    workspace_value: "",
    features: &["http"],
};
const RESOURCE: Dep = Dep {
    name: "nest-rs-resource",
    workspace_value: "",
    features: &[],
};
const GRAPHQL: Dep = Dep {
    name: "nest-rs-graphql",
    workspace_value: "",
    features: &[],
};
const WS: Dep = Dep {
    name: "nest-rs-ws",
    workspace_value: "",
    features: &[],
};
const QUEUE: Dep = Dep {
    name: "nest-rs-queue",
    workspace_value: "",
    features: &[],
};
const SCHEDULE: Dep = Dep {
    name: "nest-rs-schedule",
    workspace_value: "",
    features: &[],
};
const MCP: Dep = Dep {
    name: "nest-rs-mcp",
    workspace_value: "",
    features: &[],
};
const SEA_ORM: Dep = Dep {
    name: "sea-orm",
    workspace_value: "{ version = \"2.0.0-rc.38\", default-features = false, features = [\"sqlx-postgres\", \"runtime-tokio-rustls\", \"macros\", \"with-uuid\"] }",
    features: &[],
};
const SERDE: Dep = Dep {
    name: "serde",
    workspace_value: "{ version = \"1\", features = [\"derive\"] }",
    features: &[],
};
const UUID: Dep = Dep {
    name: "uuid",
    workspace_value: "{ version = \"1\", features = [\"v7\", \"serde\"] }",
    features: &[],
};
const VALIDATOR: Dep = Dep {
    name: "validator",
    workspace_value: "{ version = \"0.20\", features = [\"derive\"] }",
    features: &[],
};
const ASYNC_GRAPHQL: Dep = Dep {
    name: "async-graphql",
    workspace_value: "{ version = \"7\", features = [\"dataloader\"] }",
    features: &[],
};
const RMCP: Dep = Dep {
    name: "rmcp",
    workspace_value: "{ version = \"1.7\", features = [\"server\", \"macros\", \"transport-streamable-http-server\"] }",
    features: &[],
};
const ANYHOW: Dep = Dep {
    name: "anyhow",
    workspace_value: "\"1\"",
    features: &[],
};

/// The crates a resource port (DB-backed CRUD + HTTP) needs.
pub fn resource_deps() -> Vec<&'static Dep> {
    vec![&SEAORM, &RESOURCE, &SEA_ORM, &SERDE, &UUID, &VALIDATOR]
}

/// The crates an adapter for `transport` needs on top of the port.
pub fn adapter_deps(transport: Transport) -> Vec<&'static Dep> {
    match transport {
        Transport::Http => vec![],
        Transport::Graphql => vec![&GRAPHQL, &ASYNC_GRAPHQL],
        Transport::Ws => vec![&WS],
        Transport::Queue => vec![&QUEUE, &SERDE, &ANYHOW],
        Transport::Schedule => vec![&SCHEDULE, &ANYHOW],
        Transport::Mcp => vec![&MCP, &RMCP],
    }
}

/// Edit the root manifest: add any missing `[workspace.dependencies]` entries.
pub fn ensure_workspace_deps(deps: Vec<&'static Dep>) -> Transform {
    Box::new(move |content: &str| {
        let mut doc = content.parse::<DocumentMut>().ok()?;
        let table = doc["workspace"]["dependencies"]
            .or_insert(toml_edit::table())
            .as_table_mut()?;
        let mut changed = false;
        for dep in &deps {
            if table.get(dep.name).is_none() {
                table.insert(dep.name, dep.workspace_item());
                changed = true;
            }
        }
        changed.then(|| doc.to_string())
    })
}

/// Edit the `features` manifest: add any missing `[dependencies]` entries as
/// `{ workspace = true, features = [...] }`.
pub fn ensure_features_deps(deps: Vec<&'static Dep>) -> Transform {
    Box::new(move |content: &str| {
        let mut doc = content.parse::<DocumentMut>().ok()?;
        let table = doc["dependencies"]
            .or_insert(toml_edit::table())
            .as_table_mut()?;
        let mut changed = false;
        for dep in &deps {
            if table.get(dep.name).is_none() {
                table.insert(dep.name, Item::Value(features_value(dep.features)));
                changed = true;
            }
        }
        changed.then(|| doc.to_string())
    })
}

fn parse_value(raw: &str) -> Item {
    format!("x = {raw}\n")
        .parse::<DocumentMut>()
        .ok()
        .and_then(|frag| frag.get("x").cloned())
        .unwrap_or_else(|| Item::Value(Value::from(raw)))
}

fn features_value(features: &[&str]) -> Value {
    let mut table = toml_edit::InlineTable::new();
    table.insert("workspace", Value::from(true));
    if !features.is_empty() {
        let mut arr = toml_edit::Array::new();
        for f in features {
            arr.push(*f);
        }
        table.insert("features", Value::Array(arr));
    }
    Value::InlineTable(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensures_workspace_dep_idempotently() {
        let src = "[workspace.dependencies]\nnest-rs-core = \"0.1\"\n";
        let t = ensure_workspace_deps(vec![&SEAORM]);
        let out = t(src).expect("adds nest-rs-seaorm");
        // The pin tracks the CLI's own release line, not a hard-coded literal.
        assert!(out.contains(&format!("nest-rs-seaorm = \"{}\"", framework_req())));
        // already present → no-op
        assert!(ensure_workspace_deps(vec![&SEAORM])(&out).is_none());
    }

    #[test]
    fn ensures_features_dep_with_features() {
        let src = "[dependencies]\nnest-rs-core.workspace = true\n";
        let out = ensure_features_deps(vec![&SEAORM])(src).expect("adds dep");
        assert!(out.contains("nest-rs-seaorm"));
        assert!(out.contains("workspace = true"));
        assert!(out.contains("\"http\""));
    }
}
