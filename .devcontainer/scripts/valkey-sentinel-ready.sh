#!/usr/bin/env bash
# Waits until the replica follows the primary and the three sentinels name one
# primary, run by `valkey-sentinel-init` (`compose/valkey-sentinel.yml`).
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
synced() { valkey -h "$1" INFO replication | grep -E '^(role:master|master_link_status:up)' >/dev/null; }
primary() { valkey -h "$1" -p 26379 SENTINEL GET-MASTER-ADDR-BY-NAME nestrs | paste -sd: -; }
agreed() {
    local first
    first=$(primary valkey-sentinel-1)
    [ -n "$first" ] && [ "$(primary valkey-sentinel-2)" = "$first" ] && [ "$(primary valkey-sentinel-3)" = "$first" ]
}
watching() {
    valkey -h "$1" -p 26379 SENTINEL MASTER nestrs | awk '
        key == "num-slaves" { replicas = $0 }
        key == "num-other-sentinels" { sentinels = $0 }
        { key = $0 }
        END { exit !(replicas == 1 && sentinels == 2) }'
}

eventually synced valkey-sentinel-server-1
eventually synced valkey-sentinel-server-2
for sentinel in valkey-sentinel-{1..3}; do
    eventually watching "$sentinel"
done
eventually agreed
echo "Sentinel serves nestrs, its primary $(primary valkey-sentinel-1)"
