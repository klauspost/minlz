#!/usr/bin/env bash
# Run each cargo-fuzz target for $MAX_SECS (default 900 = 15 min), back to
# back, from a fresh shell.  Sources $HOME/.cargo/env so you don't need to
# `. "$HOME/.cargo/env"` first.
#
# Usage:
#   bash run.sh                       # all three, 15 min each
#   bash run.sh decode_arbitrary      # just one
#   MAX_SECS=300 bash run.sh          # 5 min each
#   JOBS=4 WORKERS=4 bash run.sh      # quieter parallelism
set -euo pipefail

# --- Env -------------------------------------------------------------------
if [[ -z "${CARGO_HOME:-}" && -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not in PATH and \$HOME/.cargo/env missing" >&2
    exit 1
fi
if ! cargo +nightly --version >/dev/null 2>&1; then
    echo "error: 'cargo +nightly' not available; run rustup toolchain install nightly" >&2
    exit 1
fi

# --- Mirror /mnt/ source tree to ext4 (WSL DrvFs is slow) ------------------
# If invoked from a Windows-drive path (/mnt/<letter>/…) we rsync the
# minlz-rs tree to $WORK_ROOT (default ~/.cache/minlz-rs-fuzz-work), cd
# there, and on exit sync corpus/ + artifacts/ back to the original
# location so crash artefacts and new corpus entries don't get stranded
# on the workdir copy.  Override with NO_MIRROR=1 to disable.
script_dir="$(cd "$(dirname "$0")" && pwd)"
WORK_ROOT="${WORK_ROOT:-$HOME/.cache/minlz-rs-fuzz-work}"
if [[ "${NO_MIRROR:-}" != "1" && "$script_dir" == /mnt/* ]]; then
    src_root="$(cd "$script_dir/../../.." && pwd)"      # minlz-rs root
    work_fuzz="$WORK_ROOT/crates/minlz/fuzz"
    echo "WSL DrvFs detected; mirroring to ext4:"
    echo "   src:  $src_root"
    echo "   work: $WORK_ROOT"
    mkdir -p "$WORK_ROOT"
    rsync -a --delete \
        --exclude /target --exclude 'crates/minlz/fuzz/target' \
        --exclude 'crates/minlz/fuzz/artifacts' \
        "$src_root/" "$WORK_ROOT/"
    sync_back() {
        local rc=$?
        # Disarm so re-entry (e.g. a second Ctrl-C) doesn't recurse.
        trap - EXIT INT TERM
        echo
        echo "Syncing corpus + artifacts back to $src_root..."
        rsync -a "$work_fuzz/corpus/"    "$script_dir/corpus/"    || true
        rsync -a "$work_fuzz/artifacts/" "$script_dir/artifacts/" || true
        echo "done."
        exit "$rc"
    }
    trap sync_back EXIT INT TERM
    cd "$work_fuzz"
else
    cd "$script_dir"
fi

# --- Config ----------------------------------------------------------------
MAX_SECS="${MAX_SECS:-900}"          # 15 min
MAX_LEN="${MAX_LEN:-4194304}"        # 4 MiB
# Default to ceil(nproc / 2) so the box stays usable for other work.
JOBS="${JOBS:-$(( ( $(nproc 2>/dev/null || echo 16) + 1 ) / 2 ))}"
WORKERS="${WORKERS:-$JOBS}"

# Targets: $1 if given, otherwise all three.
if [[ $# -ge 1 ]]; then
    targets=("$@")
else
    targets=(decode_arbitrary roundtrip max_encoded_len)
fi

# --- Helpers ---------------------------------------------------------------
hms() {
    # seconds -> H:MM:SS
    local s="$1"
    printf '%d:%02d:%02d' "$((s/3600))" "$(((s%3600)/60))" "$((s%60))"
}

count_dir() {
    [[ -d "$1" ]] && find "$1" -type f | wc -l || echo 0
}

# --- Run -------------------------------------------------------------------
total_start=$(date +%s)
echo "==============================================================="
echo " cargo-fuzz batch run"
echo "   targets : ${targets[*]}"
echo "   budget  : $(hms "$MAX_SECS") per target ($((MAX_SECS*${#targets[@]})) s total)"
echo "   jobs    : $JOBS workers, max input $MAX_LEN bytes"
echo "==============================================================="

declare -A crashes_before crashes_after seeds_before seeds_after
for tgt in "${targets[@]}"; do
    crashes_before["$tgt"]=$(count_dir "artifacts/$tgt")
    seeds_before["$tgt"]=$(count_dir "corpus/$tgt")
done

for tgt in "${targets[@]}"; do
    echo
    echo "--- $tgt -----------------------------------------------------"
    echo "  seeds: ${seeds_before[$tgt]}   prior crashes: ${crashes_before[$tgt]}"
    echo "  start: $(date '+%F %T')   budget: $(hms "$MAX_SECS")"
    t0=$(date +%s)
    set +e
    cargo +nightly fuzz run "$tgt" -- \
        -max_total_time="$MAX_SECS" \
        -jobs="$JOBS" -workers="$WORKERS" \
        -max_len="$MAX_LEN"
    status=$?
    set -e
    t1=$(date +%s)
    seeds_after["$tgt"]=$(count_dir "corpus/$tgt")
    crashes_after["$tgt"]=$(count_dir "artifacts/$tgt")
    echo "  done in $(hms "$((t1-t0))")  exit=$status"
    echo "  seeds: ${seeds_before[$tgt]} -> ${seeds_after[$tgt]}   crashes: ${crashes_before[$tgt]} -> ${crashes_after[$tgt]}"
    if [[ "${crashes_after[$tgt]}" -gt "${crashes_before[$tgt]}" ]]; then
        echo "  !! new crash(es) in artifacts/$tgt/ — see fuzz/RUNBOOK.md for triage"
    fi
done

total_end=$(date +%s)
echo
echo "==============================================================="
echo " summary  (total $(hms "$((total_end-total_start))"))"
printf "  %-20s %10s %10s %10s\n" target seeds+ crashes+ exit
for tgt in "${targets[@]}"; do
    ds=$(( ${seeds_after[$tgt]} - ${seeds_before[$tgt]} ))
    dc=$(( ${crashes_after[$tgt]} - ${crashes_before[$tgt]} ))
    printf "  %-20s %10d %10d\n" "$tgt" "$ds" "$dc"
done
echo "==============================================================="

new_crashes=0
for tgt in "${targets[@]}"; do
    new_crashes=$(( new_crashes + ${crashes_after[$tgt]} - ${crashes_before[$tgt]} ))
done
exit "$new_crashes"
