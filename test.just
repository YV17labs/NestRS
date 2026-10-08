nextest := "cargo nextest run --workspace --no-fail-fast"

# The tests of nest-rs-redis's e2e that hold on one Valkey topology alone sit in
# a module named for it; a run on another topology leaves them out.
redis := "binary_id(nest-rs-redis::e2e)"
elsewhere := redis + " and test(/(^|::)(sentinel|cluster)::/)"

# Every test: unit, integration, e2e and the doc examples
all: && doc
    {{nextest}} -E 'not ({{elsewhere}})'

# The `#[cfg(test)]` modules in each crate's `src/`
unit:
    {{nextest}} --lib --bins

# The in-process suites under each crate's `tests/`, no service needed
integration:
    {{nextest}} -E 'kind(test) and not binary(e2e)'

# The `tests/e2e` suites, against Postgres, Valkey and S3
e2e:
    {{nextest}} -E 'binary(e2e) and not ({{elsewhere}})'

# nest-rs-redis's e2e on the Valkey topology `<PREFIX>_REDIS__URL` declares —
# `sentinel` or `cluster` — its failovers last, one at a time
topology name:
    {{nextest}} -E '{{redis}} and not test(/(^|::)({{ if name == "sentinel" { "standalone|cluster" } else if name == "cluster" { "standalone|sentinel" } else { error("a topology is `sentinel` or `cluster`") } }})::/) and not test(/::failover::/)'
    {{nextest}} --test-threads 1 -E '{{redis}} and test(/(^|::){{name}}::failover::/)'

# Coverage of the nextest suite, per file; doctests count on nightly only
cov:
    cargo llvm-cov nextest --workspace --no-fail-fast

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
