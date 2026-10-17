#!/usr/bin/env bash
# Forms the Cluster, unless it is formed already, and waits until every node
# serves it with its replica in sync, run by `valkey-cluster-init`
# (`compose/valkey-cluster.yml`).
set -euo pipefail

nodes=(valkey-cluster-{1..6})
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
serving() { valkey -h "$1" CLUSTER INFO | grep '^cluster_state:ok' >/dev/null; }

if ! valkey -h "${nodes[0]}" CLUSTER INFO | grep '^cluster_slots_assigned:16384' >/dev/null; then
    valkey --cluster create "${nodes[@]/%/:6379}" --cluster-replicas 1 --cluster-yes
fi
for node in "${nodes[@]}"; do
    eventually serving "$node"
    eventually synced "$node"
done
echo "Cluster serves on ${nodes[*]}"
