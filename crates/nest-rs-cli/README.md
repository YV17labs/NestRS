# nest-rs-cli

> Part of **[NestRS](https://nestrs.dev)** (alpha). **[Documentation](https://nestrs.dev/cli/)** · [Getting started](https://nestrs.dev/getting-started/)

Scaffolding CLI for nestrs. **`nestrs new` infers the layout** from the directory tree — no mode flags.

| Situation | Command |
|-----------|---------|
| New monorepo | `nestrs new acme` → `./acme/` + `apps/hello/` on port **3000** |
| Add an app (inside a workspace) | `nestrs new billing` → `apps/billing/` — **next free port** in `module.rs` |
| Single crate | `nestrs new my-api --standalone` |

HTTP ports are pinned in each app's `module.rs` (`HttpConfig { port: … }`), not in `.env` — same pattern as `platform-auth` (3001), `platform-api` (3002), etc.

```bash
cargo install --locked nest-rs-cli just
nestrs new acme
cd acme && just dev hello
```

## Generate (workspace only)

`nestrs g` scaffolds a feature, registers it in `crates/features/src/lib.rs`, and
auto-wires the edge module into the current app's `module.rs`. All accept
`--dry-run` and `-p <path>`.

| Command | Generates |
|---------|-----------|
| `nestrs g feature <name>` | Transport-agnostic port (service + module) |
| `nestrs g resource <name>` | DB-backed CRUD port + HTTP adapter (deps auto-added) |
| `nestrs g http\|graphql\|ws\|queue\|schedule\|mcp <name>` | One transport adapter on a port |

See [nestrs.dev/cli](https://nestrs.dev/cli/) for the full reference.
