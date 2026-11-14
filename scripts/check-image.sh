#!/bin/sh
# Smoke test of the Docker image, as CI runs it: the image serves, takes the first-run
# setup, and its backup and restore commands carry that account to a new volume. It then
# serves data folders it does not own yet, without root (ADR 0025).
# Usage: scripts/check-image.sh [image], `cadocs` by default.
set -eu

image=${1:-cadocs}
name=cadocs-check-$$
port=38931
url=http://127.0.0.1:$port
# A named volume on the home of the image's user takes its owner, so that user writes there.
backup="$name-backup:/home/nonroot"

# A host folder of whoever runs this, as a bind mount an operator made.
bound=$(mktemp -d)

cleanup() {
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker volume rm -f "$name-data" "$name-restored" "$name-backup" "$name-root" \
        >/dev/null 2>&1 || true
    rm -rf "$bound"
}
trap cleanup EXIT

serve() {
    docker run -d --name "$name" -p "127.0.0.1:$port:3000" -e CADOCS_PUBLIC_URL="$url" \
        -v "$1:/data" -v "$backup" "$image" >/dev/null
    for _ in $(seq 1 50); do
        curl -fs "$url/health" >/dev/null && return 0
        sleep 0.2
    done
    docker logs "$name"
    echo "the server never answered /health" >&2
    exit 1
}

setup_required() {
    curl -fs "$url/api/v1/auth/setup"
}

# The user of the server's process, which must be $1.
serves_as() {
    uid=$(docker top "$name" -o pid,uid | awk 'END { print $2 }')
    [ "$uid" = "$1" ] || { echo "the server runs as uid $uid, not $1" >&2; exit 1; }
}

serve "$name-data"
serves_as 65532
[ "$(setup_required)" = '{"required":true}' ] || { echo "setup should be pending" >&2; exit 1; }
# The server logs the link to setup, with the code that setup needs (ADR 0021).
code=$(docker logs "$name" 2>&1 | grep -o '/setup/[0-9a-f]\{64\}' | tail -n 1 | cut -d / -f 3)
[ -n "$code" ] || { docker logs "$name"; echo "no setup link in the log" >&2; exit 1; }
status=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$url/api/v1/auth/setup" \
    -H "Origin: $url" -H 'Content-Type: application/json' \
    -d '{"email":"ada@example.org","name":"Ada","password":"correct horse battery","workspace_name":"Acme","code":"'"$code"'"}')
[ "$status" = 201 ] || { docker logs "$name"; echo "setup answered $status" >&2; exit 1; }

docker exec "$name" cadocs backup /home/nonroot/one
docker rm -f "$name" >/dev/null

docker run --rm -e CADOCS_PUBLIC_URL="$url" -v "$name-restored:/data" -v "$backup" \
    "$image" restore /home/nonroot/one
serve "$name-restored"
[ "$(setup_required)" = '{"required":false}' ] || { echo "the restore lost the account" >&2; exit 1; }
docker rm -f "$name" >/dev/null

# A folder root owns, with root's files in it, as Docker makes a bind mount that does not exist:
# the image gives it to its user. Kept in a volume, so no host file is left to root. A new volume
# takes its owner from where it is first mounted, and /mnt is root's.
docker run --rm --user 0:0 -e CADOCS_RUN_AS= -e CADOCS_DATA_DIR=/mnt -e CADOCS_PUBLIC_URL="$url" \
    -v "$name-root:/mnt" -v "$backup" "$image" restore /home/nonroot/one
serve "$name-root"
serves_as 65532
[ "$(setup_required)" = '{"required":false}' ] || { echo "root's folder lost the account" >&2; exit 1; }
docker rm -f "$name" >/dev/null

# A folder the operator made: the server runs as its owner, so the files stay theirs.
serve "$bound"
serves_as "$(id -u)"
[ "$(find "$bound" ! -user "$(id -u)" | wc -l)" = 0 ] || { echo "files of another user in $bound" >&2; exit 1; }
echo "image $image: serves, backs up and restores, and gives up root"
