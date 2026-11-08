# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.97
ARG NODE_MAJOR=24
ARG ACTIONLINT_VERSION=1.7.12
ARG HELM_VERSION=4.3.1
ARG YQ_VERSION=4.54.1

FROM mcr.microsoft.com/devcontainers/base:trixie AS dev

ARG RUST_VERSION
ARG NODE_MAJOR
ARG ACTIONLINT_VERSION
ARG HELM_VERSION
ARG YQ_VERSION

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      mold build-essential cmake clang perl pkg-config libssl-dev curl ca-certificates shellcheck \
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

USER vscode

ENV PATH=/home/vscode/.cargo/bin:/home/vscode/.local/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
          --default-toolchain "${RUST_VERSION}" \
          -c rustfmt -c clippy -c rust-analyzer -c llvm-tools-preview \
 && cargo install --locked cargo-binstall \
 && cargo binstall --no-confirm --locked \
      just bacon cargo-nextest cargo-llvm-cov cargo-deny cargo-hack zizmor

# actionlint is a Go binary cargo cannot install: its release is fetched by
# version and checked against the digests it publishes.
RUN arch="$(dpkg --print-architecture)" \
 && actionlint="actionlint_${ACTIONLINT_VERSION}_linux_${arch}.tar.gz" \
 && mkdir -p /home/vscode/.local/bin /tmp/tools && cd /tmp/tools \
 && curl -fsSLO "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/${actionlint}" \
 && curl -fsSL "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_checksums.txt" \
      | grep " ${actionlint}\$" | sha256sum -c - \
 && tar -xzf "${actionlint}" -C /home/vscode/.local/bin actionlint \
 && cd / && rm -rf /tmp/tools

# helm and yq run the chart's render cases (`nestrs run chart`), as `demo.yml` does:
# Go binaries too, each checked against the digest pinned here for its version.
RUN arch="$(dpkg --print-architecture)" \
 && case "${arch}" in \
      amd64) helm_sha256=0403a88ae20913d11b10a2467b50087721fea7581736da6b9d3992f5357fe54b \
             yq_sha256=8e34fc298390875de416e6a4afcb8cabeceb25d9aa8506c1a2f9353cf702ea5f ;; \
      arm64) helm_sha256=8cf8f66ecbc252ac8f7f06cca68aba88508ddfde3a6a34feb1678ad1694b4f88 \
             yq_sha256=189088da0c6429ec5178dfaab1a114805f6cab0b61b165ab236efedf1d57a71b ;; \
      *) echo "no helm or yq digest pinned for ${arch}" >&2; exit 1 ;; \
    esac \
 && helm="helm-v${HELM_VERSION}-linux-${arch}.tar.gz" \
 && mkdir -p /tmp/tools && cd /tmp/tools \
 && curl -fsSLO "https://get.helm.sh/${helm}" \
 && echo "${helm_sha256}  ${helm}" | sha256sum -c - \
 && tar -xzf "${helm}" -C /home/vscode/.local/bin --strip-components=1 "linux-${arch}/helm" \
 && curl -fsSL -o yq "https://github.com/mikefarah/yq/releases/download/v${YQ_VERSION}/yq_linux_${arch}" \
 && echo "${yq_sha256}  yq" | sha256sum -c - \
 && install -m 0755 yq /home/vscode/.local/bin/yq \
 && cd / && rm -rf /tmp/tools
