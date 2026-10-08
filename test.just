nextest := "cargo nextest run --workspace --no-fail-fast"

# The tests of nest-rs-redis's e2e that hold on one Valkey topology alone sit in
# a module named for it: a run keeps the tests every topology holds and its own.
redis := "binary_id(nest-rs-redis::e2e)"
on_one := "test(/(^|::)(standalone|sentinel|cluster)::/)"
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

# The dev container's `topologies` profile, as its loopback serves it.
sentinel_url := "rediss-sentinel://127.0.0.1:26379?node=127.0.0.1:26380&node=127.0.0.1:26381&sentinelServiceName=nestrs"
cluster_url := "rediss-cluster://127.0.0.1:7000?node=127.0.0.1:7001&node=127.0.0.1:7002"

# Its failovers run last, one at a time.
# nest-rs-redis's e2e on Valkey's `sentinel` or `cluster` (the `topologies` profile); CI runs neither
topology name:
    {{ if name =~ "^(sentinel|cluster)$" { "" } else { error("a topology is `sentinel` or `cluster`") } }}NESTRS_REDIS__URL='{{ if name == "sentinel" { sentinel_url } else { cluster_url } }}' {{nextest}} -E '{{redis}} and (not {{on_one}} or test(/(^|::){{name}}::/)) and not test(/::failover::/)'
    NESTRS_REDIS__URL='{{ if name == "sentinel" { sentinel_url } else { cluster_url } }}' {{nextest}} --test-threads 1 -E '{{redis}} and test(/(^|::){{name}}::failover::/)'

# Coverage of the nextest suite, per file; doctests count on nightly only
cov:
    cargo llvm-cov nextest --workspace --no-fail-fast

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
