#!/bin/sh
# Creates the buckets the development clients name: `s3://nestrs`, the
# development profile's `<PREFIX>_STORAGE__URL` and the framework suite's, and
# `s3://nestrs-test`, the demo's test profile's, so its e2e suites never file
# into what a running app reads. The storage client proves its bucket at boot
# and refuses one that does not exist, as a database is created before an app
# connects to it. A bucket kept from an earlier start keeps its objects: RustFS
# answers 200 to its owner creating it again, as S3 does in us-east-1, and a
# server answering 409 (BucketAlreadyOwnedByYou) keeps it the same.
#
#   create-bucket.sh ENDPOINT    e.g. https://rustfs:9000
#
# Signs with the pair the RustFS service starts with, and verifies its
# certificate through the system's store, where the development authority is
# installed first.
set -eu

endpoint=${1:?usage: create-bucket.sh ENDPOINT}

for bucket in nestrs nestrs-test; do
    status=$(curl -sS -o /dev/null -w '%{http_code}' \
        --aws-sigv4 'aws:amz:us-east-1:s3' \
        --user "${RUSTFS_ACCESS_KEY:-nestrs}:${RUSTFS_SECRET_KEY:-nestrs}" \
        -X PUT "$endpoint/$bucket")

    case $status in
        200 | 409) echo "bucket $bucket: ready" ;;
        *)
            echo "bucket $bucket: $endpoint answered $status to its creation" >&2
            exit 1
            ;;
    esac
done
