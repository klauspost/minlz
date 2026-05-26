# Stage B — Assembly Decoders (amd64, aarch64)

> Read `README.md` first. The project-wide rules apply.
> Stage A (safe Rust decoder) must be **DONE** before stage B starts.

## Goal

Make `minlz::block::decode` use a hand-tuned native-arch decoder
on `x86_64` and `aarch64`, matching the Go decoder assembly within
5 % on the `benchmarks_test.go` corpus. The safe Rust decoder from
stage A becomes the fallback (`cfg`-gated for unsupported targets,
big-endian, `force_safe` feature, miri).

Stage B is purely about decoder throughput. Encoders stay pure Rust
(see project-wide rule 5).

## Starting point — what stage A already shipped

Re-measure before sinking effort into hand-tuned asm.  The stage-A
safe decoder is **not** a length-exact `memcpy`-driven loop; it
already contains the biggest single optimisation that was
originally planned for stage B:

* **LZ4-style 16-byte overshoot** on the literal fast path and on
  short non-overlapping copies (`offset > length`, `length ≤ 16`,
  `offset ≥ 16`).  Implemented via `ptr::read_unaligned::<u128>` /
  `ptr::write_unaligned::<u128>` — LLVM emits a single `MOVDQU`
  on x86_64 and `LDR Q` / `STR Q` on aarch64.  See
  `crates/minlz/src/block/decode.rs::copy_short_literal` and the
  `if length <= 16 && offset >= 16 { ... }` block in `minlz_decode`.
* **Padded destination contract** — the public API allocates
  `dlen + OVERSHOOT_PAD (=16)` bytes so the overshoot is safe.
  The internal decoder entry point already takes a raw `*mut u8`
  plus the logical `dlen`; a SIMD-heavier asm path can reuse it.
* **Tight loop invariant** of `s + 16 ≤ src.len()`.  An asm
  rewrite can keep this or tighten further.

Concrete numbers from stage A (Ryzen 9 9950X, single core, Twain
1 MB):

| Decoder           | Throughput | vs Go `noasm` | vs Go `asm` (TBD) |
|-------------------|-----------:|--------------:|-------------------:|
| Go `noasm`        | 4305 MB/s  | —             | —                 |
| Rust (stage A)    | 7900 MB/s  | **+83 %**     | (re-measure)       |
| Go `asm`          | (re-measure on the same box) | | — |

**First task in stage B:** measure Go `asm` on the same machine.
If the gap to Rust is already within 5 %, much of stage B (the
hand-tuned amd64/aarch64 modules) may be unnecessary.  In that
case the deliverables collapse to:

* a `cfg`-gated module split (move `decode.rs` to
  `decode/safe.rs`, add a `mod.rs` dispatcher);
* a `force_safe` feature flag for CI miri runs;
* whatever targeted improvements close any remaining gap (likely
  the `forward_copy_ptr` byte loop for RLE patterns).

## Where the residual gap (if any) is likely to live

Profiling guidance based on the stage-A benchmark distribution:

* **`forward_copy_ptr`** (`decode.rs`).  When `offset < length`,
  each output byte depends on the previous one — currently a
  scalar byte loop.  For `offset >= 8` you can do an 8-byte
  load+store with two iterations of duplication first
  (`a → ab → abab → abababab`), then chunked 8-byte stores.  Big
  win on `zeros_64k` (currently 754 MB/s decode; bandwidth-bound
  at ~30 GB/s if vectorised).
* **Tag dispatch.**  LLVM compiles the 4-way `match tag & 0x03`
  to a jump table.  The Go `asm` uses a flat table indexed by
  the full byte; if profiling shows branch mispredictions, switch
  to a `static [fn(...); 256]` table.
* **64-byte unrolled match copy** for very long copies.  Go's asm
  uses four `MOVOU` instructions per iteration of the
  match-copy loop.  LLVM may already unroll our 16-byte loop;
  verify with `cargo asm`.

## Reference

* `asm_amd64.s` — search for `TEXT ·decodeBlockAsm(SB)` (line ~26797).
* `asm_arm64.s` — entire file (starts with the decoder; 987 lines).
* `decode.go:minLZDecodeGo` — semantic reference; the Rust safe
  decoder from stage A is the cleaner reference.
* `asm_amd64.go` — Go-side stub: `func decodeBlockAsm(dst, src []byte) int`.

## Background — what the Go decoder asm does

The amd64 decoder is essentially the inner loop of `minLZDecodeGo`
with these specific optimizations layered on:

1. **64-bit dispatch on the tag byte.** Read `src[s]` once into a
   register, mask the low 2 bits, jump-table to one of four cases.
2. **Wide literal copy.** When literal length ≤ 16, copy 16 bytes
   with `MOVUPS` regardless of true length; the inputMargin (8)
   plus output margin (17) makes the overcopy safe.
3. **Wide match copy.** For Copy2/Copy3 the spec guarantees offset
   ≥ 64, so a single 64-byte loop iteration (`MOVOU`×4) is safe
   with no overlap check.
4. **Branchless tail length.** The 3-byte length extension is
   loaded as a u32 and shifted, matching the Go fast path's
   `load32(src,s)>>8`.
5. **No-bounds-check primary, bounds-checked tail.** Same two-loop
   structure as stage A. The asm switches loops at
   `s < srcEnd - 17`.

The arm64 decoder follows the same structure but uses NEON-free
integer loads (the file at `asm_arm64.s:30+` registers R0–R17 are
all integer). NEON is not used because the wide copies are 16
bytes max and `LDP`/`STP` already give 16-byte loads.

## Implementation choices

Three options, ranked by preference:

### Option 1 — `core::arch` intrinsics in Rust (preferred)

Write the decoder in `unsafe` Rust using `std::arch::x86_64` and
`std::arch::aarch64` intrinsics for the wide copies. The compiler
generates the tight loops. This is the path zstd-rs and lz4_flex
use.

**Pros**
* No build-system complexity (no `cc` crate, no `.s` files).
* Easy to inspect generated assembly with `cargo asm`.
* Targets `cfg(target_feature = "sse2")` (x86_64 baseline) and
  `cfg(target_arch = "aarch64")` (aarch64 baseline).

**Cons**
* The compiler may not reproduce the exact 64-byte-per-iteration
  match copy from the Go asm. Mitigation: write the copy loop as
  an explicit `for _ in 0..n { unaligned_store(...) }` over `__m128i`
  or `uint8x16_t`; verify with `cargo asm`.

### Option 2 — External `.s` files via `cc`/`global_asm!`

Translate the Go `.s` to GAS or `global_asm!` syntax. Link via
`build.rs` calling `cc::Build`.

**Pros**
* Bytewise-identical asm to Go.

**Cons**
* Requires either `cc` (an extra dep) or stable `global_asm!`
  (1.59+, OK) plus `naked_functions` (still unstable as of 1.75).
* GAS vs Plan9 vs LLVM-MASM — three syntaxes for three targets.
* Hard to maintain; every change needs an asm review.

### Option 3 — Inline asm with `asm!`

Inline `asm!` macros inside Rust functions for the hot inner loops
only. The dispatch table stays in Rust.

**Pros**
* Stable as of 1.59.
* No build-system change.

**Cons**
* Still verbose; per-arch maintenance burden remains.

**Decision: start with option 1.** It is the most maintainable and,
based on the structure of the Go decoder, the compiler should
generate equivalent code. If after measurement we are >5 % slower
than the Go asm on the same machine, switch to option 3 for the
inner copies only.

## Module layout

```
crates/minlz/src/block/decode/
├── mod.rs              # pub(super) fn decode_block(dst, src) -> Result
├── safe.rs             # the stage-A decoder, renamed
├── amd64.rs            # cfg(target_arch = "x86_64")
└── aarch64.rs          # cfg(target_arch = "aarch64")
```

`mod.rs` dispatches:

```rust
pub(super) fn decode_block(dst: &mut [u8], src: &[u8]) -> Result<(), Error> {
    #[cfg(all(target_arch = "x86_64", not(feature = "force_safe")))]
    { return unsafe { amd64::decode_block_amd64(dst, src) }; }
    #[cfg(all(target_arch = "aarch64", not(feature = "force_safe")))]
    { return unsafe { aarch64::decode_block_aarch64(dst, src) }; }
    safe::decode_block(dst, src)
}
```

The `force_safe` feature exists so the test suite can exercise the
safe path on any host. The feature must be enabled in CI's safe-
path matrix entry.

## API surface (no change from stage A)

`minlz::block::decode` keeps the same signature; only the body
gets faster. No new public types.

## Testing plan

### Equivalence tests

For every test ported in stage A, add a second invocation with the
`force_safe` feature off and on, asserting bytewise-identical
output. This must run in CI on every supported target.

### Differential between safe and asm

A new test target `tests/asm_vs_safe.rs`:

* For each fixture in `crates/minlz/tests/fixtures/`, decode with
  both `decode_block_amd64` (when on amd64) / `decode_block_aarch64`
  (when on aarch64) and `safe::decode_block`. The outputs must
  match.
* Run the fuzz corpus (`testdata/fuzz/block-corpus-raw.zip`)
  through both; outputs and error verdicts must match.

This is the strongest correctness check.

### Fuzz

Extend the stage-A `decode_arbitrary` fuzz target to decode with
both safe and asm decoders and assert equality, including error
verdicts. (`Result::Ok(x) == Ok(y) && len ok`, `Err == Err`).

### Bench

Criterion benches from stage A continue to apply. The acceptance
bar tightens: now the **asm path** is what we measure, and we
target within 5 % of the Go *asm* decoder numbers from
`parallel-16.txt` (not `parallel-16-noasm.txt`).

Also benchmark `force_safe` to verify stage A hasn't regressed.

### Miri

Miri does not understand inline SIMD intrinsics. The miri job in
CI uses `--features force_safe`. Mark all `unsafe` blocks in the
amd64/aarch64 modules with `// SAFETY:` so a human review can
verify them.

### Cross-arch verification

CI matrix:

| Job            | Target              | Decoder under test            |
|----------------|---------------------|-------------------------------|
| linux-amd64    | x86_64-unknown-linux| amd64 + safe (via feature)    |
| linux-arm64    | aarch64-unknown-linux| aarch64 + safe               |
| macos-arm64    | aarch64-apple-darwin| aarch64 + safe                |
| windows-amd64  | x86_64-pc-windows-msvc| amd64 + safe                |
| miri           | x86_64-unknown-linux| safe only (force_safe)       |
| linux-riscv64  | (optional)          | safe (no asm path; verify cfg gate)|

`cargo cross` can be used for the riscv64 sanity check.

## Risks

* The Go `MOVOU` 64-byte unroll may not be reproducible with
  Rust's `_mm_storeu_si128` because LLVM may unroll differently.
  If `cargo asm` shows a less-tight loop, switch to inline `asm!`
  for that one inner loop. Time-box this work: 2 days to get
  within 5 %, then escalate.
* `aarch64` weak memory model is not relevant here — the decoder
  is single-threaded over its `dst`/`src`. But the `Result` return
  must use `core::sync::atomic::compiler_fence` only if we
  multi-thread later (we don't, in stage B).
* Cross-arch testing requires CI runners we may not have today.
  At minimum: linux-amd64 and macos-arm64 cover both arches; add
  windows-amd64 because that's the dev host.

## Success criteria (DONE = all of these)

1. All stage-A test/fuzz/bench criteria still pass on every
   supported target.
2. `tests/asm_vs_safe.rs` passes on amd64 and aarch64 CI runners
   for the full fuzz corpus.
3. Decoder bench numbers are within 5 % of the Go asm decoder on
   the same machine for the testFiles corpus.
4. `force_safe` build + test still passes, gating against future
   regressions in the portable fallback.
5. No `unsafe` block without a `// SAFETY:` comment.
6. CI has at least three host arches green: amd64-linux,
   amd64-windows, aarch64-{macos|linux}.

## Open questions

1. **`is_x86_feature_detected!` at runtime vs `target_feature`
   compile-time?** Go uses build-time only. Match Go: assume
   baseline x86_64-v1 (SSE2) and aarch64. Skip runtime detection
   to keep dispatch cheap.
2. **AVX2 path?** The Go decoder does not use AVX2; do not add it. Probably no benefit.
3. **arm64 `LDP`/`STP` 16-byte ops via intrinsics?** They are
   `vld1q_u8` / `vst1q_u8`. Use them for the wide copy.
4. **Should we strip the safe decoder from release builds when the
   target supports asm?** No — keep it linked behind `force_safe`
   so users can opt out for auditing.
5. **PPC64LE, RISCV64, LoongArch64.** All currently fall through
   to the safe path. Document this in `lib.rs` rustdoc; do not
   implement asm for them in stage B.
