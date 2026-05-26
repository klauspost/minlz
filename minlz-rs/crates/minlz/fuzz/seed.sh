#!/usr/bin/env bash
# Extract the Go reference repo's fuzz corpora into cargo-fuzz's
# `corpus/<target>/` directories.  Run once before `cargo +nightly fuzz run`.
#
# Usage:
#   GO_REPO=/path/to/go/minlz  bash seed.sh
# or, if the Go and Rust repos are siblings:
#   bash seed.sh
set -euo pipefail

# Resolve $GO_REPO.  Layouts we try, in order:
#   1. $GO_REPO env override.
#   2. Four levels up — minlz-rs is currently nested inside the Go repo
#      ($here = minlz/minlz-rs/crates/minlz/fuzz, $here/../../../.. = minlz/).
#   3. Sibling layout: $here/../../../../minlz — once minlz-rs becomes a
#      stand-alone repo and lives next to the Go repo.
here="$(cd "$(dirname "$0")" && pwd)"
candidates=(
    "${GO_REPO:-}"
    "$here/../../../.."
    "$here/../../../../minlz"
)
for c in "${candidates[@]}"; do
    [[ -z "$c" ]] && continue
    if [[ -d "$c/testdata/fuzz" ]]; then
        GO_REPO="$(cd "$c" && pwd)"
        break
    fi
done
if [[ -z "${GO_REPO:-}" || ! -d "$GO_REPO/testdata/fuzz" ]]; then
    echo "error: cannot find Go repo testdata/fuzz/.  Tried:" >&2
    for c in "${candidates[@]}"; do
        [[ -n "$c" ]] && echo "  $c" >&2
    done
    echo "Set GO_REPO=/path/to/go/minlz and rerun." >&2
    exit 2
fi
echo "Using Go repo: $GO_REPO"

cd "$(dirname "$0")"
mkdir -p corpus/decode_arbitrary corpus/roundtrip corpus/max_encoded_len

# decode_arbitrary — raw byte corpus from the Go FuzzDecodeBlock seeds.
unzip -o -q "$GO_REPO/testdata/fuzz/block-corpus-raw.zip"  -d corpus/decode_arbitrary
unzip -o -q "$GO_REPO/testdata/fuzz/block-corpus-dec.zip"  -d corpus/decode_arbitrary
# Encoded blocks also exercise the decoder.
unzip -o -q "$GO_REPO/testdata/fuzz/block-corpus-enc.zip"  -d corpus/decode_arbitrary

# roundtrip + max_encoded_len — encode random plaintexts, so seed with the
# raw corpus + the encoder-regression set.
unzip -o -q "$GO_REPO/testdata/fuzz/block-corpus-raw.zip"  -d corpus/roundtrip
unzip -o -q "$GO_REPO/testdata/enc_regressions.zip"        -d corpus/roundtrip
unzip -o -q "$GO_REPO/testdata/fuzz/block-corpus-raw.zip"  -d corpus/max_encoded_len
unzip -o -q "$GO_REPO/testdata/enc_regressions.zip"        -d corpus/max_encoded_len

# Optional: also pull in the live Go fuzz cache (`go test -fuzz=...` saves
# every coverage-discovered input here).  Much richer than the static
# block-corpus-*.zip seeds.  Honours $GOCACHE_FUZZ env override; otherwise
# derives from `go env GOCACHE`.
if [[ -z "${GOCACHE_FUZZ:-}" ]] && command -v go >/dev/null 2>&1; then
    gc="$(go env GOCACHE 2>/dev/null || true)"
    if [[ -n "$gc" ]] && [[ -d "$gc/fuzz/github.com/minio/minlz" ]]; then
        GOCACHE_FUZZ="$gc/fuzz/github.com/minio/minlz"
    fi
fi
if [[ -n "${GOCACHE_FUZZ:-}" && -d "$GOCACHE_FUZZ" ]]; then
    echo "Importing Go fuzz cache: $GOCACHE_FUZZ"
    helper_dir="$(dirname "$0")/_seed_helper"
    # Map Go fuzz function -> cargo-fuzz target.  Stream fuzzers belong to
    # stage C and are skipped here.
    declare -A go_to_rust=(
        ["FuzzDecodeBlock"]="decode_arbitrary"
        ["FuzzEncodingBlocks"]="roundtrip"
    )
    for go_fn in "${!go_to_rust[@]}"; do
        rust_target="${go_to_rust[$go_fn]}"
        gosrc="$GOCACHE_FUZZ/$go_fn"
        [[ -d "$gosrc" ]] || continue
        (cd "$helper_dir" && go run . "$gosrc" "$(pwd)/../corpus/$rust_target")
    done
    # Encoder regression cache seeds the size + roundtrip targets equally.
    if [[ -d "$GOCACHE_FUZZ/FuzzEncodingBlocks" ]]; then
        (cd "$helper_dir" && go run . "$GOCACHE_FUZZ/FuzzEncodingBlocks" \
            "$(pwd)/../corpus/max_encoded_len")
    fi
else
    echo "(skipping Go fuzz cache import — set GOCACHE_FUZZ to enable)"
fi

for d in corpus/decode_arbitrary corpus/roundtrip corpus/max_encoded_len; do
    n="$(find "$d" -type f | wc -l)"
    echo "$d: $n seeds"
done
