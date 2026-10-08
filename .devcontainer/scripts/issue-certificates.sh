#!/bin/sh
# Issues the one TLS certificate every development service presents — in the
# dev container and in CI — and hands each service its own copy.
#
#   issue-certificates.sh DIR
#
#   DIR/ca.pem              the authority every client trusts
#   DIR/<service>/<files>   one folder per service: the certificate and its
#                           key, under the names and the owner it reads
#
# The certificate is kept while it has a month left, every service has its copy
# and it serves client authentication too — a Valkey Cluster's nodes present it
# to each other on the cluster bus, which always asks for one; otherwise a new
# authority issues a new one, and the authority's key is thrown away. Runs as
# root, to give each service the owner it needs.
#
# Adding a service: one row below. Its name is its hostname on the compose
# network, and joins the certificate's names on its own.
set -eu

# service                 uid    certificate      key              how the service reads them
SERVICES='
postgres                  999    server.pem       server.key       postgres -c ssl_cert_file / -c ssl_key_file
valkey-standalone         999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-sentinel-server-1  999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-sentinel-server-2  999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-sentinel-1         999    server.pem       server.key       valkey-sentinel --tls-cert-file / --tls-key-file
valkey-sentinel-2         999    server.pem       server.key       valkey-sentinel --tls-cert-file / --tls-key-file
valkey-sentinel-3         999    server.pem       server.key       valkey-sentinel --tls-cert-file / --tls-key-file
valkey-cluster-1          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-cluster-2          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-cluster-3          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-cluster-4          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-cluster-5          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
valkey-cluster-6          999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
rustfs                    10001  rustfs_cert.pem  rustfs_key.pem   RUSTFS_TLS_PATH, which fixes the two names
'

dir=${1:?usage: issue-certificates.sh DIR}

services() {
    echo "$SERVICES" | while read -r service uid cert key _; do
        if [ -n "$service" ]; then
            echo "$service $uid $cert $key"
        fi
    done
}

# Every service's copy present, and good for a month: kept.
fresh() {
    [ -f "$dir/ca.pem" ] || return 1
    services | while read -r service uid cert key; do
        [ -f "$dir/$service/$key" ] || exit 1
        openssl x509 -checkend 2592000 -noout -in "$dir/$service/$cert" >/dev/null 2>&1 || exit 1
        openssl x509 -noout -ext extendedKeyUsage -in "$dir/$service/$cert" 2>/dev/null \
            | grep -q 'TLS Web Client Authentication' || exit 1
    done
}

# One line per service: what became of its copy, and until when it holds.
report() {
    services | while read -r service _ cert _; do
        expiry=$(openssl x509 -enddate -dateopt iso_8601 -noout -in "$dir/$service/$cert")
        echo "$service: certificate $1, valid until ${expiry#notAfter=}"
    done
}

if fresh; then
    report kept
    exit 0
fi

# The certificate names each service, and the loopback a CI runner reaches
# them on.
names="DNS:localhost,IP:127.0.0.1,IP:::1"
for service in $(services | cut -d ' ' -f 1); do
    names="DNS:$service,$names"
done

echo "no certificate good for another month in $dir: issuing one for $names"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# openssl writes a lone `-----` to stderr even when it succeeds: its stderr is
# shown only when it fails.
quietly() {
    "$@" 2>"$work/stderr" || {
        cat "$work/stderr" >&2
        return 1
    }
}

quietly openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
    -subj "/CN=nestrs development authority" \
    -addext "basicConstraints=critical,CA:true" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout "$work/ca.key" -out "$work/ca.pem"
quietly openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 365 \
    -CA "$work/ca.pem" -CAkey "$work/ca.key" \
    -subj "/CN=nestrs development services" \
    -addext "subjectAltName=$names" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=serverAuth,clientAuth" \
    -keyout "$work/key.pem" -out "$work/cert.pem"

install -d -m 0755 "$dir"
install -m 0644 "$work/ca.pem" "$dir/ca.pem"
services | while read -r service uid cert key; do
    install -d -m 0755 "$dir/$service"
    install -m 0644 -o "$uid" -g "$uid" "$work/cert.pem" "$dir/$service/$cert"
    install -m 0600 -o "$uid" -g "$uid" "$work/key.pem" "$dir/$service/$key"
done
report issued
