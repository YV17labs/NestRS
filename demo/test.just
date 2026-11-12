_default:
    @just --list test

# Unit + integration + doctests, no DB.
unit:
    cargo nextest run --workspace -E 'not binary(e2e)'
    cargo test --workspace --doc

# e2e tests (live Postgres/Redis), gated by the nextest binary filter.
e2e:
    cargo nextest run --workspace -E 'binary(e2e)'

# Coverage on the full suite (includes the e2e binary).
cov:
    cargo llvm-cov nextest --workspace --no-report
    cargo llvm-cov report
    cargo llvm-cov report --lcov --output-path lcov.info

# Doctests only
doc:
    cargo test --workspace --doc
