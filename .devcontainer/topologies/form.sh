#!/usr/bin/env bash
# Forms the Cluster and waits for the sentinels to agree, once the `topologies`
# profile's nodes answer: run by its `topologies` service, on the dev
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
serving() { valkey -p "$1" CLUSTER INFO | grep '^cluster_state:ok' >/dev/null; }
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

if ! valkey -p 7000 CLUSTER INFO | grep '^cluster_slots_assigned:16384' >/dev/null; then
    valkey --cluster create 127.0.0.1:{7000..7005} --cluster-replicas 1 --cluster-yes
fi
for port in {7000..7005}; do
    eventually serving "$port"
    eventually synced "$port"
done

eventually synced 6380
eventually synced 6381
for port in 26379 26380 26381; do
    eventually watching "$port"
done
eventually agreed
echo "Sentinel and Cluster serve on 127.0.0.1"
