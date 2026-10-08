set allow-duplicate-variables

nextest := "cargo nextest run --workspace --no-fail-fast"

# Where nest-rs-redis's e2e finds Valkey: the URL CI sets, else the dev
# container's one server. With the `valkey-sentinel` or `valkey-cluster` dev
# container open, uncomment its line.
redis_url := env_var_or_default("NESTRS_REDIS__URL", "rediss://valkey-standalone:6379")
# redis_url := "rediss-sentinel://valkey-sentinel-1:26379?node=valkey-sentinel-2:26379&node=valkey-sentinel-3:26379&sentinelServiceName=nestrs"
# redis_url := "rediss-cluster://valkey-cluster-1:6379?node=valkey-cluster-2:6379&node=valkey-cluster-3:6379"

# Its tests that hold on one topology alone sit in a module named for it, and a
# failover in a `failover` module.
topology := if redis_url =~ '^rediss?-sentinel://' { "sentinel" } else if redis_url =~ '^rediss?-cluster://' { "cluster" } else { "standalone" }
redis := "binary_id(nest-rs-redis::e2e)"
elsewhere := redis + " and test(/(^|::)(standalone|sentinel|cluster)::/) and not test(/(^|::)" + topology + "::/)"
failovers := redis + " and test(/(^|::)" + topology + "::failover::/)"

# Every test: unit, integration, e2e and the doc examples
all: && doc
    #!/usr/bin/env bash
    set -euo pipefail
    export NESTRS_REDIS__URL='{{redis_url}}'
    {{nextest}} -E 'not ({{elsewhere}}) and not ({{failovers}})'
    # A failover moves the primaries every other test runs on: last, one at a time.
    if [ "{{topology}}" != standalone ]; then
        {{nextest}} --test-threads 1 -E '{{failovers}}'
    fi

# The `#[cfg(test)]` modules in each crate's `src/`
unit:
    {{nextest}} --lib --bins

# The in-process suites under each crate's `tests/`, no service needed
integration:
    {{nextest}} -E 'kind(test) and not binary(e2e)'

# The `tests/e2e` suites, against Postgres, Valkey and S3
e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    export NESTRS_REDIS__URL='{{redis_url}}'
    {{nextest}} -E 'binary(e2e) and not ({{elsewhere}}) and not ({{failovers}})'
    if [ "{{topology}}" != standalone ]; then
        {{nextest}} --test-threads 1 -E '{{failovers}}'
    fi

# Coverage of the nextest suite, per file; doctests count on nightly only
cov:
    cargo llvm-cov nextest --workspace --no-fail-fast

# The examples in the docs
doc:
    cargo test --workspace --doc --no-fail-fast
