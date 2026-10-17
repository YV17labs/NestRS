#!/usr/bin/env bash
# Run by every dev container environment at each start: the development
# authority joins the system's store, so every client trusts the services'
# certificate as it trusts any other authority; then the bucket the
# development clients name is created, unless kept from an earlier start.
set -euo pipefail

sudo install -m 0644 /certs/ca.pem /usr/local/share/ca-certificates/nestrs-development.crt
sudo update-ca-certificates
sh .devcontainer/scripts/create-bucket.sh https://rustfs:9000
