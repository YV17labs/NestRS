nextest := "cargo nextest run --workspace --no-fail-fast"

# Every test: unit, integration, e2e and the doc examples
all: && doc
    {{nextest}}

# The `#[cfg(test)]` modules in each crate's `src/`
unit:
    {{nextest}} --lib --bins

# The in-process suites under each crate's `tests/`, no service needed
integration:
    {{nextest}} -E 'kind(test) and not binary(e2e)'

# The `tests/e2e` suites, against Postgres, Redis and S3
e2e:
    {{nextest}} -E 'binary(e2e)'

# CI runs it on the oldest Redis and Valkey the docs claim.
# nest-rs-redis's suites alone, against the Redis `<PREFIX>_REDIS__URL` names
redis:
    cargo nextest run -p nest-rs-redis --all-features --no-fail-fast

# Coverage of the nextest suite, per file; doctests count on nightly only
cov:
    cargo llvm-cov nextest --workspace --no-fail-fast

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
