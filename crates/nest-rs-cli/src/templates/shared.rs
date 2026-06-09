//! Files shared by standalone and workspace scaffolds (env cascade, gitignore, …).

pub const RUST_TOOLCHAIN: &str = r#"[toolchain]
channel = "1.95"
"#;

pub const GITIGNORE: &str = r#"/target
**/*.rs.bk

# Coverage (cargo-llvm-cov)
*.profraw
*.profdata
/coverage

# Local secrets (see `.env.example`)
.env.local
.env.*.local

# Editor / OS
.idea/
*.swp
.DS_Store
"#;

pub const DOCKERIGNORE: &str = r#"target/
.git/
.env.local
.env.*.local
"#;

pub const ENV: &str = r#"# {{env_label}} — committed base config (`.env` cascade).
#
# Only overrides live here; omitted keys use in-code defaults. Real environment
# variables always win. Per-machine secrets go in `.env.local` (git-ignored);
# see `.env.example`.
#
# Precedence (highest first):
#   real env  >  .env.<NESTRS_ENV>.local  >  .env.local  >  .env.<NESTRS_ENV>  >  .env

# HTTP server listen port (default: 3000).
NESTRS_HTTP__PORT=3000
"#;

/// Workspace root `.env` — no HTTP port; each app pins its port in `module.rs`.
pub const ENV_WORKSPACE: &str = r#"# {{env_label}} — committed base config (`.env` cascade).
#
# HTTP listen ports live in each app's root `module.rs`
# (`HttpConfig { port: …, ..Default::default() }`), not here.
#
# Precedence (highest first):
#   real env  >  .env.<NESTRS_ENV>.local  >  .env.local  >  .env.<NESTRS_ENV>  >  .env
"#;

pub const ENV_DEVELOPMENT: &str = r#"# {{env_label}} — development-only overrides (NESTRS_ENV=development, the default).
# Committed; layered on top of `.env`, below `.env.local` and the real environment.

# Verbose, human-readable logs while developing.
NESTRS_OPENTELEMETRY__LOG_LEVEL=debug
NESTRS_OPENTELEMETRY__LOG_FORMAT=text
"#;

pub const ENV_EXAMPLE: &str = r#"# Copy to `.env.local` for machine-specific or secret-shaped settings:
#
#   cp .env.example .env.local
#
# Uncomment when you add a database (https://nestrs.dev/configuration/).

# NESTRS_DATABASE__URL=postgres://user:pass@localhost:5432/{{kebab}}
# NESTRS_QUEUE__URL=redis://localhost:6379
"#;
