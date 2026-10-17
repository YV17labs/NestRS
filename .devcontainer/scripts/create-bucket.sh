#!/bin/sh
# Creates the bucket every development client names (`s3://nestrs`, the test
# and development profiles' `<PREFIX>_STORAGE__URL`): the storage client proves
# it at boot and refuses one that does not exist, as a database is created
# before an app connects to it. A bucket kept from an earlier start answers
# 409, and is kept.
#
#   create-bucket.sh ENDPOINT    e.g. https://rustfs:9000
#
# Signs with the pair the RustFS service starts with, and verifies its
# certificate through the system's store, where the development authority is
# installed first.
set -eu

endpoint=${1:?usage: create-bucket.sh ENDPOINT}
bucket=nestrs

status=$(curl -sS -o /dev/null -w '%{http_code}' \
    --aws-sigv4 'aws:amz:us-east-1:s3' \
    --user "${RUSTFS_ACCESS_KEY:-nestrs}:${RUSTFS_SECRET_KEY:-nestrs}" \
    -X PUT "$endpoint/$bucket")

case $status in
    200) echo "bucket $bucket: created" ;;
    409) echo "bucket $bucket: kept" ;;
    *)
        echo "bucket $bucket: $endpoint answered $status to its creation" >&2
        exit 1
        ;;
esac
