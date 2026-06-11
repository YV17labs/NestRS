# Database lifecycle recipes, exposed as `nestrs run db <verb>` (see `mod db` in
# the root Justfile). Verbs mirror SeaORM's migrate subcommands; the tools
# resolve DatabaseConfig from the `.env` cascade (NESTRS_DATABASE__*), same as
# the apps.

# Bare `nestrs run db` lists these instead of running the first recipe.
_default:
    @just --list db

# Apply all pending migrations
up:
    cargo run -p migrations --bin migrate -- up

# Roll back the last applied migration
down:
    cargo run -p migrations --bin migrate -- down

# Drop every table and re-apply all migrations from scratch
fresh:
    cargo run -p migrations --bin migrate -- fresh

# Show which migrations are applied vs. pending
status:
    cargo run -p migrations --bin migrate -- status

# Seed demo data (idempotent)
seed:
    cargo run -p seed --bin seed

# Clean slate: drop, re-migrate, then reseed
reset: fresh seed
