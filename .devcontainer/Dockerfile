# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.97
ARG NODE_MAJOR=24
ARG ACTIONLINT_VERSION=1.7.12

FROM mcr.microsoft.com/devcontainers/base:trixie AS dev

ARG RUST_VERSION
ARG NODE_MAJOR
ARG ACTIONLINT_VERSION

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      mold build-essential cmake clang perl pkg-config libssl-dev curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gnupg \
 && mkdir -p /etc/apt/keyrings \
 && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
      | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
 && printf 'Types: deb\nURIs: https://deb.nodesource.com/node_%s.x/\nSuites: nodistro\nComponents: main\nSigned-By: /etc/apt/keyrings/nodesource.gpg\n' "${NODE_MAJOR}" \
      > /etc/apt/sources.list.d/nodesource.sources \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

# actionlint, pinned by checksum as CI pins it, so `just workflows` runs here.
RUN arch=$(dpkg --print-architecture) \
 && case "$arch" in \
      amd64) sum=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8 ;; \
      arm64) sum=325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6 ;; \
      *) echo "no pinned actionlint for $arch" >&2; exit 1 ;; \
    esac \
 && curl -fsSL -o /tmp/actionlint.tar.gz \
      "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_linux_${arch}.tar.gz" \
 && echo "$sum  /tmp/actionlint.tar.gz" | sha256sum --check --strict \
 && tar -xzf /tmp/actionlint.tar.gz -C /usr/local/bin actionlint \
 && rm /tmp/actionlint.tar.gz

USER vscode

ENV PATH=/home/vscode/.cargo/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
          --default-toolchain "${RUST_VERSION}" \
          -c rustfmt -c clippy -c rust-analyzer -c llvm-tools-preview \
 && cargo install --locked cargo-binstall \
 && cargo binstall --no-confirm --locked \
      just bacon cargo-nextest cargo-llvm-cov cargo-outdated \
      cargo-deny@0.20 cargo-machete@0.9 cargo-hack@0.6 zizmor@1.30
