//! Files shared by standalone and workspace scaffolds (env cascade, gitignore, …).

pub const RUST_TOOLCHAIN: &str = r#"[toolchain]
channel = "1.95"
"#;

/// `db.just` — shipped in every project so the database verbs are present from
/// day one, whether or not the project has a database yet. Recipes follow the
/// nestrs convention: a `migrations` crate (the `migrate` bin) and a `seed` crate.
/// They start working once you add those — see the database docs.
pub const DB_JUSTFILE: &str = r#"# Database lifecycle, exposed as `nestrs run db <verb>` (see `mod db` in the
# Justfile). Recipes assume the nestrs `migrations` + `seed` crates.

# Bare `nestrs run db` lists these instead of running the first recipe.
_default:
    @just --list db

# Apply all pending migrations.
up:
    cargo run -p migrations --bin migrate -- up

# Roll back the last applied migration (`nestrs run db down 3` reverts the last 3).
down n='1':
    cargo run -p migrations --bin migrate -- down {{n}}

# Drop every table and re-apply all migrations from scratch.
fresh:
    cargo run -p migrations --bin migrate -- fresh

# Show which migrations are applied vs. pending.
status:
    cargo run -p migrations --bin migrate -- status

# Seed demo data (idempotent).
seed:
    cargo run -p seed --bin seed

# Clean slate: drop, re-migrate, then reseed.
reset: fresh seed
"#;

/// `compose.yml` — Postgres + Redis for local development, shipped so a
/// DB-backed feature works the moment you add one: `docker compose up -d`,
/// then `nestrs run db up`. The committed `.env` points at these services on
/// `localhost`. Delete it if your project never touches a database or a queue.
pub const COMPOSE: &str = r#"# Local development services. Start them with:
#
#   docker compose up -d
#
# The committed `.env` points NESTRS_DATABASE__URL / NESTRS_QUEUE__URL at these
# on localhost. `nestrs run db up` then applies your migrations.

services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: {{kebab}}
      POSTGRES_PASSWORD: {{kebab}}
      POSTGRES_DB: {{kebab}}
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U {{kebab}}"]
      interval: 5s
      timeout: 3s
      retries: 10

  redis:
    image: redis:7
    ports:
      - "6379:6379"
    volumes:
      - redisdata:/data

volumes:
  pgdata:
  redisdata:
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
# Postgres + Redis as `compose.yml` exposes them on localhost. Start them with
# `docker compose up -d`, then `nestrs run db up`. An app only connects if it
# imports DatabaseModule / a queue module, so these are inert for a plain HTTP app.
NESTRS_DATABASE__URL=postgres://{{kebab}}:{{kebab}}@localhost:5432/{{kebab}}
NESTRS_QUEUE__URL=redis://localhost:6379
#
# Precedence (highest first):
#   real env  >  .env.<NESTRS_ENV>.local  >  .env.local  >  .env.<NESTRS_ENV>  >  .env
"#;

pub const ENV_DEVELOPMENT: &str = r#"# {{env_label}} — development-only overrides (NESTRS_ENV=development, the default).
# Committed; layered on top of `.env`, below `.env.local` and the real environment.

# Verbose, human-readable logs while developing.
NESTRS_OPENTELEMETRY__LOG_LEVEL=debug
NESTRS_OPENTELEMETRY__LOG_FORMAT=text
NESTRS_OPENTELEMETRY__LOG_SOURCE_LOCATION=true
"#;

pub const ENV_EXAMPLE: &str = r#"# Copy to `.env.local` for machine-specific or secret-shaped settings:
#
#   cp .env.example .env.local
#
# Uncomment when you add a database (https://nestrs.dev/configuration/).

# NESTRS_DATABASE__URL=postgres://user:pass@localhost:5432/{{kebab}}
# NESTRS_QUEUE__URL=redis://localhost:6379
"#;
