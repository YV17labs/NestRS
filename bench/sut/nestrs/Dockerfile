# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.96

FROM rust:${RUST_VERSION}-slim-trixie AS builder
WORKDIR /repo
COPY crates crates
COPY bench/sut/nestrs bench/sut/nestrs
WORKDIR /repo/bench/sut/nestrs
RUN cargo build --release

FROM gcr.io/distroless/cc-debian13:nonroot AS runtime
COPY --from=builder /repo/bench/sut/nestrs/target/release/sut-nestrs /usr/local/bin/app
EXPOSE 3100
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/app"]
