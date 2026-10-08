#!/usr/bin/env bash
# Run by every dev container environment at each start: the development
# authority joins the system's store, so every client trusts the services'
# certificate as it trusts any other authority.
set -euo pipefail

sudo install -m 0644 /certs/ca.pem /usr/local/share/ca-certificates/nestrs-development.crt
sudo update-ca-certificates
