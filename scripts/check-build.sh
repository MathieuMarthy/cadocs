#!/bin/sh
# What a build may contain (ROADMAP V1 exit): a default build has no S3 code, an `s3` build has it,
# and no release binary passes 30 MB. As CI runs it (docs/notes/ci.md).
# Usage: scripts/check-build.sh <default|s3>           the server's dependency graph, by cargo tree
#        scripts/check-build.sh <default|s3> <binary>  the size and the S3 code of a release binary
set -eu

flavor=${1:?usage: scripts/check-build.sh <default|s3> [binary]}
binary=${2:-}
case $flavor in
    default) features= ;;
    s3) features=--features=cadocs-server/s3 ;;
    *) echo "unknown flavor $flavor" >&2; exit 2 ;;
esac
limit=30000000
# The crates only the S3 store brings in (ADR 0020).
crates='rusty-s3 reqwest'
# Panic locations of those crates and of the S3 store, which a stripped binary still holds. The
# `s3` flavor must show them, or the check proves nothing.
markers='rusty-s3 reqwest cadocs-files/src/s3'

if [ -z "$binary" ]; then
    # Normal edges only: `cadocs-files` has an in-memory S3 among its dev-dependencies.
    tree=$(cargo tree -p cadocs-server -e normal --prefix none $features)
    for crate in $crates; do
        if echo "$tree" | grep -q "^$crate v"; then found=yes; else found=no; fi
        case $flavor/$found in
            default/yes) echo "a default build depends on $crate" >&2; exit 1 ;;
            s3/no) echo "an s3 build does not depend on $crate" >&2; exit 1 ;;
        esac
    done
    echo "$flavor: dependencies as expected"
    exit 0
fi

[ -f "$binary" ] || { echo "no binary at $binary" >&2; exit 1; }
size=$(wc -c <"$binary")
[ "$size" -lt "$limit" ] || { echo "$binary is $size bytes, over $limit" >&2; exit 1; }
for marker in $markers; do
    if grep -q -a -F "$marker" "$binary"; then found=yes; else found=no; fi
    case $flavor/$found in
        default/yes) echo "$binary holds S3 code: $marker" >&2; exit 1 ;;
        s3/no) echo "$binary has no $marker, so this check no longer sees S3 code" >&2; exit 1 ;;
    esac
done
echo "$flavor: $binary is $size bytes, S3 code as expected"
