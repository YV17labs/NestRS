#!/bin/sh
# Issues the one TLS certificate every development service presents — in the
# dev container and in CI — and hands each service its own copy.
#
#   issue.sh DIR
#
#   DIR/ca.pem              the authority every client trusts
#   DIR/<service>/<files>   one folder per service: the certificate and its
#                           key, under the names and the owner it reads
#
# The certificate is kept while it has a month left and every service has its
# copy; otherwise a new authority issues a new one, and the authority's key is
# thrown away. Runs as root, to give each service the owner it needs.
#
# Adding a service: one row below. Its name is its hostname on the compose
# network, and joins the certificate's names on its own.
set -eu

# service  uid    certificate      key              how the service reads them
SERVICES='
postgres   999    server.pem       server.key       postgres -c ssl_cert_file / -c ssl_key_file
redis      999    server.pem       server.key       valkey-server --tls-cert-file / --tls-key-file
rustfs     10001  rustfs_cert.pem  rustfs_key.pem   RUSTFS_TLS_PATH, which fixes the two names
'

dir=${1:?usage: issue.sh DIR}

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
    done
}
if fresh; then
    exit 0
fi

# The certificate names each service, and the loopback a CI runner reaches
# them on.
names="DNS:localhost,IP:127.0.0.1,IP:::1"
for service in $(services | cut -d ' ' -f 1); do
    names="DNS:$service,$names"
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
    -subj "/CN=nestrs development authority" \
    -addext "basicConstraints=critical,CA:true" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout "$work/ca.key" -out "$work/ca.pem" 2>/dev/null
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 365 \
    -CA "$work/ca.pem" -CAkey "$work/ca.key" \
    -subj "/CN=nestrs development services" \
    -addext "subjectAltName=$names" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=serverAuth" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null

install -d -m 0755 "$dir"
install -m 0644 "$work/ca.pem" "$dir/ca.pem"
services | while read -r service uid cert key; do
    install -d -m 0755 "$dir/$service"
    install -m 0644 -o "$uid" -g "$uid" "$work/cert.pem" "$dir/$service/$cert"
    install -m 0600 -o "$uid" -g "$uid" "$work/key.pem" "$dir/$service/$key"
done
