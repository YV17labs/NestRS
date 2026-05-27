# Database lifecycle recipes, exposed as `just db <verb>` (see `mod db` in the
# root Justfile). Verbs mirror SeaORM's migrate subcommands; all read DATABASE_URL.

# Bare `just db` lists these instead of running the first recipe.
_default:
    @just --list db

# Apply all pending migrations
up:
    cargo run -p db --bin migrate -- up

# Roll back the last applied migration
down:
    cargo run -p db --bin migrate -- down

# Drop every table and re-apply all migrations from scratch
fresh:
    cargo run -p db --bin migrate -- fresh

# Show which migrations are applied vs. pending
status:
    cargo run -p db --bin migrate -- status

# Seed demo data (idempotent)
seed:
    cargo run -p db --bin seed

# Clean slate: drop, re-migrate, then reseed
reset: fresh seed
