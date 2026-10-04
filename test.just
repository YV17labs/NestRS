nextest := "cargo nextest run --workspace --no-fail-fast"

# Every test: unit, integration, e2e and the doc examples
all: && doc
    {{nextest}}

# The `#[cfg(test)]` modules in each crate's `src/`
unit:
    {{nextest}} --lib --bins

# The suites under each crate's `tests/`
integration:
    {{nextest}} --test '*'

# The `tests/e2e` suites, against Postgres, Redis and S3
e2e:
    {{nextest}} -E 'binary(e2e)'

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
