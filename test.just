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

# The `tests/e2e` suites, against Postgres, Valkey and S3
e2e:
    {{nextest}} -E 'binary(e2e)'

# Coverage of the nextest suite, per file; doctests count on nightly only
cov:
    cargo llvm-cov nextest --workspace --no-fail-fast

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
