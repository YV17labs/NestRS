#!/usr/bin/env bash
# Run once by every dev container environment when it is created: installs the
# `nestrs` CLI of this tree.
set -euo pipefail

cargo install --locked --path crates/nest-rs-cli
