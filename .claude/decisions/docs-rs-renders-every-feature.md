# docs.rs renders every feature of a crate that declares one

2026-11-17. docs.rs builds a crate's default features alone, so an item behind
any other feature, and every link naming it, is missing from the crate's page.
Seven published crates set `all-features = true` under
`[package.metadata.docs.rs]`; five declaring features did not, and
nest-rs-core's page showed `Secret` without the SeaORM column impls `seaorm`
gates.

Every published crate declaring a feature sets `all-features = true`, so its
page shows what `just doc` checks: every feature, warnings denied.

**Refused:**

- **The table only where a page loses an item today.** Four of the five pages
  were whole without it: nest-rs-http's `websocket` gates tier-2 items alone,
  nest-rs-opentelemetry's `otlp` and nest-rs-reqwest's `middleware` are default
  features, and nest-rs-seaorm-macros' `graphql` changes an expansion, not an
  item. Which items a feature gates is a judgment every new feature would have
  to remake, and five crates without the table show it is not remade.
- **A `features = [...]` list per crate**: it drifts from `[features]` with every
  feature added, which `all-features` cannot.
