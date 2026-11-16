#!/bin/sh
# Makes the upgrade fixture of a release: the data folder that release's own binary leaves after
# a fixed set of requests, which `crates/cadocs-server/tests/upgrade.rs` opens with the current
# build. Run it once per release, after the release is published (docs/notes/ci.md).
# Usage: scripts/make-upgrade-fixture.sh <tag>, for example v0.2.0. Needs curl, jq and sqlite3.
set -eu

tag=${1:?usage: scripts/make-upgrade-fixture.sh <tag>}
repo=https://github.com/MathieuMarthy/cadocs
out=crates/cadocs-server/tests/upgrade/$tag
port=38932
url=http://127.0.0.1:$port
# The password of `tests/common/mod.rs`, which the test logs in with.
password="correct horse battery"
# Bytes the test compares with the download.
png='\211PNG\r\n\032\nupgrade fixture'

[ -e "$out" ] && { echo "$out already exists" >&2; exit 1; }
work=$(mktemp -d)
pid=
cleanup() {
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

# The default build of the release, as an operator downloads it.
name=cadocs-$tag-$(uname -m)-unknown-linux-gnu
curl -fsSL -o "$work/$name.tar.gz" "$repo/releases/download/$tag/$name.tar.gz"
curl -fsSL -o "$work/$name.tar.gz.sha256" "$repo/releases/download/$tag/$name.tar.gz.sha256"
(cd "$work" && sha256sum -c "$name.tar.gz.sha256" >/dev/null && tar -xzf "$name.tar.gz")
binary=$work/$name/cadocs

data=$work/data
mkdir "$data"
# Revisions come quickly, so the one of each import is in the fixture.
CADOCS_DATA_DIR=$data CADOCS_PUBLIC_URL=$url CADOCS_BIND=127.0.0.1:$port \
    CADOCS_REVISION_PAUSE_SECS=1 CADOCS_REVISION_MAX_SECS=2 \
    "$binary" >"$work/log" 2>&1 &
pid=$!
for _ in $(seq 1 50); do
    curl -fs "$url/health" >/dev/null && break
    sleep 0.2
done
curl -fs "$url/health" >/dev/null || { cat "$work/log"; echo "no answer on /health" >&2; exit 1; }

# `call <cookie jar> <method> <path> [body]` prints the JSON answer, and fails on an HTTP error.
call() {
    jar=$1 method=$2 path=$3
    shift 3
    csrf=$(cat "$jar.csrf" 2>/dev/null || true)
    curl -fsS -b "$jar" -c "$jar" -X "$method" -H "origin: $url" -H "x-csrf-token: $csrf" \
        -H 'content-type: application/json' "$url/api/v1$path" ${1:+--data-binary "$1"}
}
# `send <cookie jar> <path> <content type> <file>` posts a raw body, as uploads and imports are.
send() {
    csrf=$(cat "$1.csrf")
    curl -fsS -b "$1" -c "$1" -X POST -H "origin: $url" -H "x-csrf-token: $csrf" \
        -H "content-type: $3" "$url/api/v1$2" --data-binary "@$4"
}
remember_csrf() {
    jq -r .csrf_token >"$1.csrf"
}

ada=$work/ada bob=$work/bob
code=$(grep -o '/setup/[0-9a-f]\{64\}' "$work/log" | tail -n 1 | cut -d / -f 3)
body=$(jq -n --arg p "$password" --arg c "$code" \
    '{email: "ada@example.org", name: "Ada", password: $p, workspace_name: "Acme", code: $c}')
call "$ada" POST /auth/setup "$body" | remember_csrf "$ada"
workspace=$(call "$ada" GET /session | jq -r '.workspaces[0].id')

# Bob joins as a member through an invitation, and edits the space by a grant.
link=$(call "$ada" POST "/workspaces/$workspace/invitations" \
    '{"email": "bob@example.org", "role": "member"}' | jq -r .link)
body=$(jq -n --arg p "$password" '{name: "Bob", password: $p}')
call "$bob" POST "/invitations/${link##*/}/accept" "$body" | remember_csrf "$bob"
bob_id=$(call "$bob" GET /session | jq -r .user.id)

space=$(call "$ada" POST "/workspaces/$workspace/spaces" \
    '{"name": "Handbook", "description": "How we work", "default_role": "viewer"}' | jq -r .id)
call "$ada" PUT "/spaces/$space/grants" \
    "$(jq -n --arg u "$bob_id" '{grants: [{user_id: $u, role: "editor"}]}')"

printf '# Welcome\n\nThe **quokka** team keeps its notes here.\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n' \
    >"$work/welcome.md"
welcome=$(send "$ada" "/spaces/$space/import?name=welcome.md" text/markdown "$work/welcome.md" \
    | jq -r .id)
printf "$png" >"$work/chart.png"
image=$(send "$ada" "/pages/$welcome/attachments?name=chart.png" image/png "$work/chart.png" \
    | jq -r .id)
printf '# Onboarding\n\nStart at [the welcome page](/?page=%s).\n\n![the chart](/api/v1/attachments/%s)\n' \
    "$welcome" "$image" >"$work/onboarding.md"
send "$bob" "/spaces/$space/import?name=onboarding.md&parent_id=$welcome" text/markdown \
    "$work/onboarding.md" >/dev/null
printf '# Old notes\n\nNothing here.\n' >"$work/old.md"
old=$(send "$ada" "/spaces/$space/import?name=old.md" text/markdown "$work/old.md" | jq -r .id)
call "$ada" DELETE "/pages/$old"

# The search index and the revisions are written by jobs, after the imports.
for _ in $(seq 1 100); do
    found=$(call "$ada" GET "/workspaces/$workspace/search?q=quokka" | jq '.items | length')
    revisions=$(call "$ada" GET "/pages/$welcome/revisions" | jq '.items | length')
    [ "$found" -ge 1 ] && [ "$revisions" -ge 1 ] && break
    sleep 0.2
done
[ "$found" -ge 1 ] || { echo "the search never found the page" >&2; exit 1; }
[ "$revisions" -ge 1 ] || { echo "the page never got a revision" >&2; exit 1; }

kill "$pid"
wait "$pid" || true
pid=
# The database as that release left it, with nothing left in a WAL.
sqlite3 "$data/cadocs.db" 'PRAGMA wal_checkpoint(TRUNCATE);' >/dev/null
rm -f "$data/cadocs.db-wal" "$data/cadocs.db-shm"
mkdir -p "$out"
cp "$data/cadocs.db" "$out/"
cp -r "$data/files" "$out/"
echo "wrote $out"
