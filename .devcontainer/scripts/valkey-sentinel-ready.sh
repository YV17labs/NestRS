#!/usr/bin/env bash
# Waits until the replica follows the primary and the three sentinels name one
# primary, run by `valkey-sentinel-init` (`compose/valkey-sentinel.yml`) on the dev
# container's loopback.
set -euo pipefail

valkey() { timeout 60 valkey-cli --tls --cacert /certs/ca.pem "$@"; }
eventually() {
    for _ in {1..120}; do
        if "$@"; then return 0; fi
        sleep 0.5
    done
    echo "still false after a minute: $*" >&2
    return 1
}
synced() { valkey -p "$1" INFO replication | grep -E '^(role:master|master_link_status:up)' >/dev/null; }
primary() { valkey -p "$1" SENTINEL GET-MASTER-ADDR-BY-NAME nestrs | paste -sd: -; }
agreed() {
    local first
    first=$(primary 26379)
    [ -n "$first" ] && [ "$(primary 26380)" = "$first" ] && [ "$(primary 26381)" = "$first" ]
}
watching() {
    valkey -p "$1" SENTINEL MASTER nestrs | awk '
        key == "num-slaves" { replicas = $0 }
        key == "num-other-sentinels" { sentinels = $0 }
        { key = $0 }
        END { exit !(replicas == 1 && sentinels == 2) }'
}

eventually synced 6380
eventually synced 6381
for port in 26379 26380 26381; do
    eventually watching "$port"
done
eventually agreed
echo "Sentinel serves on 127.0.0.1"
