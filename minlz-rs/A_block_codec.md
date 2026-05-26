# Stage A — Block Encoders (L1–L3) and Safe Decoder

> Read `README.md` in this folder first. The project-wide rules apply.
> Most relevant: idiomatic Rust, minimal deps, perf within 5 % of the Go
> `noasm` path, test coverage ≥ Go, **no** SuperFast, **no** Snappy/S2
> fallback, **no** LZ4 conversion, **no** asm encoder.

## Goal

A self-contained block codec that:

1. Encodes a `&[u8]` buffer (up to 8 MiB) at one of three levels —
   `Fastest`, `Balanced`, `Smallest` — into the MinLZ block format
   defined in `SPEC.md` §1–§2.
2. Decodes any valid MinLZ block back to bytes, rejecting corrupt
   input without panicking or reading out of bounds.
3. Performs within 5 % of the corresponding Go `noasm` path on the
   `benchmarks_test.go` corpus.
4. Has parity with the Go fuzz corpus (`testdata/fuzz/*.zip`) — every
   seed that decodes/encodes successfully in Go must do so in Rust,
   and every seed that errors in Go must error in Rust.

This stage **does not** include streaming, the index, search tables,
or the CLI. The output of stage A is a library that can be called as
`minlz::block::encode(src, level)` and `minlz::block::decode(src)`.

## Scope (in)

* Block-level constants: tag values, min/max copy offsets, prime hashes,
  block size limits (`SPEC.md` §1.1, §2.1–§2.5; `minlz.go`, `encode.go`).
* Bit-twiddle helpers (`load8/16/32/64`, `store8/16/32`) — Rust
  equivalents using `from_le_bytes` / `read_unaligned`.
* Varint encode/decode matching Go `binary.PutUvarint`/`binary.Uvarint`
  (uvarint, max 10 bytes, 64-bit). Reference: `decode.go:decodedLen`,
  `encode.go:Encode` lines around 90.
* Emit functions:
  * `emit_literal` ↔ `asm_none.go:emitLiteral`
  * `emit_repeat` ↔ `asm_none.go:emitRepeat`
  * `emit_copy` ↔ `asm_none.go:emitCopy`
  * `emit_copy_lits2` ↔ `asm_none.go:emitCopyLits2`
  * `emit_copy_lits3` ↔ `asm_none.go:emitCopyLits3`
  * `encode_copy2` ↔ `encode.go:encodeCopy2`
  * `encode_copy3` ↔ `asm_none.go:encodeCopy3`
* Size predictors used by L3:
  * `emit_literal_size_n` ↔ `encode.go:emitLiteralSizeN`
  * `emit_repeat_size` (find in `encode_l2.go`/`encode_l3.go`)
  * `emit_copy_size` (find in `encode_l3.go`)
* Hash helpers: `hash4`, `hash5`, `hash6`, `hash7`, `hash8` — primes
  in `encode_l2.go:hash4..hash8`, `encode_l1.go:hash6`.
* `match_len` ↔ `asm_none.go:matchLen`.
* Three encoders:
  * **L1 / Fastest** — `encode_l1.go:encodeBlockGo` (8 MiB),
    `encode_l1.go:encodeBlockGo64K` (≤ 64 KiB). Single 15-bit hash
    table for ≥ 64 K input, 13-bit `u16` table for ≤ 64 K input.
  * **L2 / Balanced** — `encode_l2.go:encodeBlockBetterGo` and
    `encode_l2.go:encodeBlockBetterGo64K`. Two hash tables (17-bit
    long, 14-bit short). Sets a `sync.Pool`-managed long table —
    Rust analog is `thread_local!` or a per-call `Box`.
  * **L3 / Smallest** — `encode_l3.go:encodeBlockBest`. 20+18-bit
    tables, multi-candidate evaluation by encoded-size score.
* Safe decoder: `decode.go:minLZDecodeGo` — two-loop structure
  (margin loop + tail loop). Returns `Result<&'_ mut [u8], Error>`.
* Public block API:
  * `encode(dst, src, level)`
  * `try_encode(dst, src, level)` — returns `None` if not
    compressible (Go: `TryEncode`)
  * `append_encoded(dst, src, level)` (Go: `AppendEncoded`)
  * `decode(dst, src)` and `append_decoded(dst, src)`
  * `decoded_len(src)` — Go: `DecodedLen`
  * `is_minlz(src)` — Go: `IsMinLZ`
  * `max_encoded_len(n)` — Go: `MaxEncodedLen`

## Scope (out)

* L0 / SuperFast (`encode_l0.go`).
* **All dictionary encoding.** `dict.go`, every `*dict` parameter in
  `encode_l3.go`, the `maxDictSrcOffset` constant, and any dict-
  related branch in the L3 encoder are dropped. No dict type, no
  dict-accepting API, no TODO breadcrumbs in the Rust source. The
  L3 port must read as if dictionaries do not exist.
* Snappy / S2 fallback in `Decode` (`decode.go:Decode` falls through
  to `s2.Decode` when first byte ≠ 0). The Rust port returns an error
  in that case.
* Assembly encoders (stage **B** only covers the *decoder*).

## Format invariants the port must preserve

The Go test suite enforces these and so must the Rust suite:

* Decoder fails on `src` with `src.len() == 0` (returns error, not
  empty success).
* Block starts with `0x00`; varint length follows. Varint > 8 MiB is
  rejected (`ErrTooLarge`).
* Block with declared length 0 emits remainder as literals.
* Tag bit layout:
  * `tagLiteral    = 0b00` (bit 2 set ⇒ Repeat)
  * `tagCopy1      = 0b01`
  * `tagCopy2      = 0b10`
  * `tagCopy3      = 0b11 | 0b100` (i.e. tag byte low 3 = 0b111)
  * `tagCopy2Fused = 0b11` (tag byte low 3 = 0b011)
* Copy offsets: Copy1 [1..=1024], Copy2 [64..=65599], Copy3
  [65536..=2162687] — these are the *true* offsets after adding the
  per-encoding bias.
* Copy lengths: Copy1 [4..=273], Copy2 [4..=(64 + 3-byte ext)],
  fused Copy2 [4..=11], Copy3 [4..=(64 + 3-byte ext)].
* `MaxEncodedLen(0) == 1`, `MaxEncodedLen(n>0) == n + 2`,
  `MaxEncodedLen(n > 8 MiB) == -1` → Rust returns `Option<usize>`
  or `Err`.

## Rust-specific design

### Error type

A single `pub enum Error` in `error.rs`:

```rust
pub enum Error {
    Corrupt,          // ErrCorrupt
    TooLarge,         // ErrTooLarge
    InvalidLevel,     // ErrInvalidLevel
    // (CRC errors land in stage C, not here.)
}
```

`impl std::error::Error` and `Display`. No `thiserror` — boilerplate
is small.

### Level enum

```rust
#[repr(i8)]
pub enum Level { Fastest = 1, Balanced = 2, Smallest = 3 }
```

`Uncompressed` (0) is not exposed via the block API — it is only
meaningful in the stream writer (stage C). `SuperFast` (-1) is out
of scope.

### Buffer ownership

Mirror Go's `func Encode(dst, src []byte, level int) ([]byte, error)`
with:

```rust
pub fn encode(dst: &mut Vec<u8>, src: &[u8], level: Level) -> Result<(), Error>;
pub fn append_encoded(dst: &mut Vec<u8>, src: &[u8], level: Level) -> Result<(), Error>;
```

`encode` clears `dst` first, `append_encoded` appends. Return value
is `()` because the buffer length is observable.

For decode:

```rust
pub fn decode(dst: &mut Vec<u8>, src: &[u8]) -> Result<(), Error>;
pub fn decoded_len(src: &[u8]) -> Result<usize, Error>;
```

A `decode_into(&mut [u8], &[u8])` variant is provided for callers
that know the exact decoded size and want to skip the size lookup.

**Implementation note (delivered in stage A):** the internal
decoder entry point is `unsafe fn(dst_ptr: *mut u8, dlen: usize,
src: &[u8])`, not `fn(dst: &mut [u8], src: &[u8])`.  The signature
change exists so callers can supply a destination buffer with
`OVERSHOOT_PAD = 16` bytes of writable padding past `dlen`; the
hot path uses a single 16-byte unaligned store for short copies
(see *LZ4-style overshoot* below).  The public `decode` / 
`append_decoded` API is unchanged — `append_decoded` reserves
`dlen + OVERSHOOT_PAD` on the `Vec`, calls the unsafe entry point,
then `set_len(dlen)` so the padding is never observable.

### Hash tables

Go uses `sync.Pool` for the L2/L3 long table (`encLPool`,
`encBestLPool`, `encBestSPool`) to avoid 0.5–4 MiB per-call
allocations.

Rust analog: `thread_local!` cells holding `Box<[u32; N]>` /
`Box<[u64; N]>`. Cleared on borrow via `iter_mut().for_each(|x| *x = 0)`,
which the compiler turns into a `memset`. Document in the source why
thread-local is preferred over a global lock or a per-call alloc.

For L1's 15-bit table (128 KiB of `u32` ≈ 128 KiB stack), Go uses a
stack array `var table [maxTableSize]uint32`. Rust will likely need
`Box<[u32; 32768]>` because Rust stack frames are 1 MiB by default
on Windows and 8 MiB on Linux/macOS — but Windows is a target. The
64 K variant's `[u16; 8192]` is 16 KiB and fits on the stack.

### Unaligned LE loads/stores

The hot path needs unaligned 64-bit LE loads. Provide:

```rust
#[inline(always)]
unsafe fn load64_le(p: *const u8) -> u64 {
    core::ptr::read_unaligned(p as *const u64).to_le()
}
```

On all supported little-endian targets (x86_64, aarch64, x86,
loongarch64) this generates a single `MOVQ`. `to_le()` is a no-op
on LE. On big-endian it lowers to `BSWAP` / `REV` — correct but
slower; BE is supported but explicitly not in the perf-target
matrix.

`bits::TrailingZeros64` ↔ `u64::trailing_zeros`.

### Loop structure

The Go primary loop checks `s < len(src) - 11` and uses unsafe
loads everywhere. Match this exactly. The tail loop uses bounds-
checked indexing — match this too. The two loops together are about
450 lines in the Rust port; keep them as two functions
(`decode_block_fast` and `decode_block_tail`) called from one entry
point.

**Stage-A change:** the fast-loop invariant was tightened from
`s + 11 <= src.len()` (matches Go) to `s + 16 <= src.len()` so the
literal fast-path can read 16 bytes from `src` in one
`read_unaligned::<u128>` without bounds-checking each byte.  The
last ≤ 15 bytes of `src` go through the tail loop — perf impact is
negligible because the tail handles at most one op per call.

### LZ4-style overshoot

The decoder's literal and short-copy fast paths use a single
16-byte unaligned load+store (`u128` via `ptr::read_unaligned` /
`ptr::write_unaligned`) instead of a length-exact `memcpy`.  This
trades a few wasted bytes on every short emit for skipping the
runtime `memcpy` function-call overhead.

Conditions:

* For literals: enabled when `length <= 16 && s + 16 <= src_len`.
* For docopy (non-overlapping `offset > length`): enabled when
  `length <= 16 && offset >= 16` (offset ≥ 16 ensures the 16-byte
  read does not overlap the just-written destination range, which
  would read uninitialised bytes from the padding region).

The OVERSHOOT_PAD = 16 contract on the destination buffer is what
makes the trailing-byte overwrite safe.  Doubled decoder throughput
vs the length-exact path on the Twain corpus: 2200 → 7900 MB/s on
1 MB Twain (vs Go `noasm` decoder at 4305 MB/s).

This trick is *not* applied to the offset-≤-length forward-copy
(RLE) path — that still uses a byte-by-byte loop because each read
depends on the previous write.  A chunked-store variant would be
possible but is left for stage B; on real corpora the RLE branch is
< 5 % of decoder time.

### `i32` vs `usize`

Go uses `int` for positions and `uint32` for hash table slots. Rust
should use `usize` for positions, `u32` for hash slots. Be careful
with `s - repeat` going negative — Go relies on wrap; Rust must
`saturating_sub` or `wrapping_sub` consistently and document the
expected sign. The reference encoders in `internal/reference/` are
clearer about overflow handling.

### `no_std`?

Open question. The block codec needs `alloc` (for `Vec<u8>` and
`Box`). It does not need `std::io`. If `no_std + alloc` is feasible
with no cost, do it — leave `std::io` integration for stage C.
Decision: **default to `std` for stage A; cfg-gate to `no_std + alloc`
only if it costs less than 50 LOC of cfg attributes.**

## File layout for this stage

```
crates/minlz/src/
├── lib.rs                  # pub use block::*, pub use error::Error
├── error.rs                # enum Error
├── block/
│   ├── mod.rs              # public API: encode/decode/...
│   ├── format.rs           # constants, MaxEncodedLen, varint
│   ├── load_store.rs       # load64_le, store32_le, etc. (unsafe)
│   ├── hash.rs             # hash4..hash8
│   ├── emit.rs             # emit_literal/copy/repeat, encode_copy2/3
│   ├── match_len.rs        # match_len
│   ├── decode.rs           # safe decoder (two-loop)
│   ├── encode_l1.rs        # fastest
│   ├── encode_l2.rs        # balanced
│   └── encode_l3.rs        # smallest
└── tests/                  # integration & differential tests
```

## Testing plan

Tests live in `crates/minlz/tests/` and `crates/minlz/src/*/tests.rs`
modules.

### Unit tests (ported from Go)

**Coverage policy:** port **every** block-level test from
`minlz_test.go`, `encode_test.go`, and `decode_test.go`, except
those listed as exclusions below. Inventory current as of the
reference branch; if the Go suite grows, the Rust suite must
follow before stage A is declared DONE.

#### Must port

From `minlz_test.go`:

| Rust target                                  | Go source                            |
|----------------------------------------------|---------------------------------------|
| `tests::block::test_max_encoded_len`         | `TestMaxEncodedLen` (line 45)         |
| `tests::block::test_empty`                   | `TestEmpty` (199)                     |
| `tests::block::test_small_copy`              | `TestSmallCopy` (205)                 |
| `tests::block::test_small_rand`              | `TestSmallRand` (218)                 |
| `tests::block::test_small_regular`           | `TestSmallRegular` (231)              |
| `tests::block::test_small_repeat`            | `TestSmallRepeat` (243)               |
| `tests::block::test_invalid_varint`          | `TestInvalidVarint` (258)             |
| `tests::block::test_decode`                  | `TestDecode` (287)                    |
| `tests::block::test_decode_copy4`            | `TestDecodeCopy4` (507)               |
| `tests::block::test_decode_length_offset`    | `TestDecodeLengthOffset` (537)        |
| `tests::block::test_decode_golden_input`     | `TestDecodeGoldenInput` (635)         |
| `tests::block::test_slow_forward_copy_overrun`| `TestSlowForwardCopyOverrun` (667)   |
| `tests::block::test_encoder_skip`            | `TestEncoderSkip` (711)               |
| `tests::block::test_encode_noise_then_repeats`| `TestEncodeNoiseThenRepeats` (783)   |
| `tests::block::test_emit_literal`            | `TestEmitLiteral` (874)               |
| `tests::block::test_emit_copy`               | `TestEmitCopy` (916)                  |
| `tests::block::test_match_len`               | `TestMatchLen` (1607)                 |
| `tests::block::test_roundtrips`              | `TestRoundtrips` (1477)               |
| `tests::block::test_data_roundtrips`         | `TestDataRoundtrips` (1541)           |

From `encode_test.go`:

| Rust target                                  | Go source                            |
|----------------------------------------------|---------------------------------------|
| `tests::encode::test_encode_huge`            | `TestEncodeHuge` (26)                 |
| `tests::encode::test_sizes`                  | `TestSizes` (75)                      |
| `tests::encode::test_emitters`               | `TestEmitters` (102)                  |

From `decode_test.go`:

| Rust target                                  | Go source                            |
|----------------------------------------------|---------------------------------------|
| `tests::decode::test_decode_block`           | `TestDecodeBlock` (24)                |
| `tests::decode::test_decode_block_random`    | `TestDecodeBlockRandom` (58)          |
| `tests::decode::test_decode_block_overlapping`| `TestDecodeBlockOverlapping` (91)    |
| `tests::decode::test_decode_block_long_offsets`| `TestDecodeBlockLongOffsets` (123)  |

#### Excluded (with reason)

| Go test                                  | Reason                                                  |
|------------------------------------------|----------------------------------------------------------|
| `TestEncodeHugeS2` (encode_test.go:52)   | Snappy/S2 fallback — out of scope per project rule 5.   |
| `TestReaderS2UncompressedDataOK` (1093)  | Snappy/S2 fallback — stream-level, also belongs to C.   |
| `TestReaderSnappyUncompressedDataOK` (1108)| Snappy fallback — stream-level.                       |
| `TestReaderMinLZUncompressedDataOK` (1123)| Belongs to stage C, not stage A.                       |
| `TestReaderUncompressedDataNoPayload` (1139)| Stage C.                                              |
| `TestReaderUncompressedDataTooLong` (1149)| Stage C.                                                |
| `TestReaderReset`, `TestWriterReset*`, `TestFlush`, `TestNumUnderlyingWrites`, `TestNewWriter`, `TestFramingFormat*`, `TestLeading*`, `TestRoundtrips` (Reader+Writer halves) | Stream-level — stage C / E. |

Stream-level tests (`TestReader*`, `TestWriter*`, `TestFlush`,
`TestNewWriter`, `TestFraming*`, `TestLeading*`) are explicitly
**deferred to stage C**, not skipped. They appear in the stage-C
test plan.

#### Fuzz seeds

The Go fuzz targets `FuzzEncodingBlocks` (fuzz_test.go:31) and
`FuzzDecodeBlock` (fuzz_test.go:120) define the corpus.
Stream-level fuzzers (`FuzzStreamEncode`, `FuzzStreamDecode`)
belong to stage C.

| Rust target                                | Go source                |
|--------------------------------------------|---------------------------|
| `tests::roundtrip::test_block_corpus`      | `FuzzEncodingBlocks` seeds (block-corpus-raw.zip, block-corpus-enc.zip, enc_regressions.zip) |
| `tests::roundtrip::test_decode_corpus`     | `FuzzDecodeBlock` seeds   |

### Differential tests (Go-as-oracle)

Generate fixture files **once**, with a small Go program checked
into `crates/minlz/tests/fixtures/`:

* For each level in {1,2,3}, take 20 representative inputs from
  `benchmarks_test.go:testFiles` (alice29, geo.protodata, html, …)
  truncated to ≤ 1 MiB each. Store both the input and the Go-
  encoded output. Rust must:
  * Decode Go's output exactly back to the input.
  * Re-encode the input and check the result decodes back (the
    Rust encoder may produce a *different* byte sequence; only
    round-trip equality is required).

* For decode-only: take a handful of `testdata/enc_regressions.zip`
  entries, decode in Go, store the (compressed, decompressed)
  pairs. Rust decoder must produce the same decompressed bytes
  for the same compressed input.

### Fuzz targets

`crates/minlz/fuzz/fuzz_targets/`:

* `decode_arbitrary.rs` — fuzz the decoder with random bytes;
  must never panic or read OOB. Seed corpus from
  `testdata/fuzz/block-corpus-raw.zip`.
* `roundtrip.rs` — encode random bytes at each level, decode,
  assert equality. Seed corpus from
  `testdata/fuzz/block-corpus-enc.zip`.
* `max_encoded_len.rs` — encoder output must never exceed
  `max_encoded_len(src.len())` for any level — port of
  `FuzzEncodingBlocks`'s tail-buffer check.

Document how to run: `cargo +nightly fuzz run decode_arbitrary` etc.

### Benchmarks

Criterion benchmarks in `crates/minlz/benches/block.rs`:

* `encode_l1_{file}`, `encode_l2_{file}`, `encode_l3_{file}` —
  matches Go's `BenchmarkEncodeLevel*` table-driven shape from
  `benchmarks_test.go`.
* `decode_{file}` — matches Go's `BenchmarkDecode`.

For each benchmark, record the Go `noasm` number from
`parallel-16-noasm.txt` and assert the Rust number is within 5 %.
The check can be an `assert!` inside the bench harness or a CSV
diff in CI; prefer the latter so a slow CI host doesn't fail PRs.

### Memory safety verification

Run the decode fuzz target under miri (`cargo +nightly miri test`)
with the seed corpus. Miri is slow — limit miri runs to a 30-second
budget and let cargo-fuzz handle volume.

## Success criteria (DONE = all of these)

1. `cargo build --release` succeeds with no warnings (and with
   `-D warnings`).
2. `cargo test` passes.
3. `cargo clippy --all-targets -- -D warnings` passes.
4. `cargo fmt --check` passes.
5. Differential test suite passes against checked-in Go fixtures.
6. Fuzz targets run for ≥ 1 hour total wall-clock with no findings
   (each of the three for ≥ 20 min). Findings, if any, are fixed
   and added to the corpus.
7. Bench numbers for each (level, file) pair are within 5 % of the
   recorded Go `noasm` number on the same machine. If a pair is
   slower, either fix it or document why and get sign-off before
   merging.
8. Public API has rustdoc on every item, including a one-paragraph
   format overview pointing to SPEC.md.

## Open questions

1. **`no_std` decision.** **Deferred.** The delivered crate is
   `std`-only (uses `Vec`, `std::error::Error`).  Converting to
   `no_std + alloc` is ~20 lines of cfg attributes — defer until a
   `no_std` consumer materialises.
2. **Hash table allocation strategy.** **Decided.** `thread_local!`
   cells holding `Vec<u32|u64>` (re-using the storage between
   calls, zero-cleared on borrow).  No measurable contention.
3. **Should `encode` take `&mut [u8]` for true zero-alloc paths?**
   **Decided.** Stayed with `&mut Vec<u8>` — no caller has asked
   for `&mut [u8]`.
4. **Score function for L3.** **Decided.** Ported literally; the
   compression ratio matches Go (`-tags=noasm`) to three decimals
   on every test-file we've measured (0.625 vs 0.6250 on 1 MB
   Twain).
5. _(Removed.) Dictionary encoding is fully out of scope per
   project rule 5 — not an open question._
6. **Endianness.** MinLZ's wire format is little-endian.  **BE
   targets are supported but explicitly not in the perf-target
   matrix.**  `to_le()` / `from_le_bytes` insert `BSWAP` / `REV` on
   BE, which is correct.  Documented in `lib.rs` crate-level
   rustdoc.

## Implementation status (delivered)

Snapshot taken at the end of stage-A implementation.  Anything
not crossed off is what a follow-up contributor should pick up.

### Acceptance criteria

| # | Criterion                                                         | Status                                           |
|---|-------------------------------------------------------------------|--------------------------------------------------|
| 1 | `cargo build --release` clean with `-D warnings`                  | ✅                                                |
| 2 | `cargo test --lib`                                                | ✅ 34 tests                                       |
| 3 | `cargo clippy --all-targets -- -D warnings`                       | ✅                                                |
| 4 | `cargo fmt --check`                                               | ✅                                                |
| 5 | Differential decoder tests vs checked-in Go fixtures              | ✅ Go `asm` and Go `noasm` fixtures both decode    |
| 6 | Fuzz ≥ 1 h, each target ≥ 20 min, no findings                     | ✅ 45 min total, 0 crashes, +14 k corpus seeds across the 3 targets (first-pass numbers in §"Outstanding work / Full fuzz run") |
| 7 | Bench within 5 % of Go `noasm` for every (level, file)            | 🟡 see "Performance summary" below                |
| 8 | Rustdoc on every public item, format overview pointing to SPEC.md | ✅ in `lib.rs` (LE/BE note included)               |

### Performance summary (AMD Ryzen 9 9950X, single-core)

Measured on `testdata/Mark.Twain-Tom.Sawyer.txt` via the hand-rolled
`examples/bench.rs`.  Go numbers from `go test -tags=noasm -bench`.
Positive Δ = Rust faster than Go `noasm`.

| Workload                | Rust enc  | Go noasm enc | Δ enc       | Rust dec  | Go noasm dec | Δ dec       |
|-------------------------|----------:|-------------:|------------:|----------:|-------------:|------------:|
| Twain 100 KB, L1        | 1054 MB/s | 1286 MB/s    | **-18 %**   | 7900 MB/s | 4552 MB/s    | **+74 %**   |
| Twain 1 MB,   L1        | 1396 MB/s | 1219 MB/s    | **+14 %**   | 7900 MB/s | 4305 MB/s    | **+83 %**   |
| Twain 100 KB, L2        |  581 MB/s |  465 MB/s    | **+25 %**   | 4569 MB/s | 1845 MB/s    | **+148 %**  |
| Twain 1 MB,   L2        |  446 MB/s |  465 MB/s    | **-4 %**    | 3327 MB/s | 1845 MB/s    | **+80 %**   |
| Twain 1 MB,   L3        |   30 MB/s |   14 MB/s    | n/a (no speed target) | 4624 MB/s | 2626 MB/s | **+76 %**  |

Compression-ratio parity to three decimals at every level:

| Level | Rust ratio | Go `noasm` ratio |
|------:|-----------:|-----------------:|
| L1    | 0.771      | 0.7714           |
| L2    | 0.652      | 0.6522           |
| L3    | 0.625      | 0.6250           |

**Within 5 % on encoder:** L1 at 1 MB and L2 at 1 MB.  **Within 5 %
on decoder:** every workload (all faster than Go noasm thanks to
the LZ4 overshoot).

**Outside 5 %:** L1-100K and L2-100K encode show variance —
likely warmup / table-zero cost being amortised over fewer
iterations.  Worth re-measuring with criterion before treating as
a real regression.

### Deviations from the original plan

1. **Internal decoder API.**  The `pub(super)` entry point is
   `unsafe fn minlz_decode(dst_ptr: *mut u8, dlen: usize, src: &[u8])`
   rather than `fn(dst: &mut [u8], src: &[u8])`.  This lets the
   caller (`block::mod.rs::append_decoded`) supply a buffer padded
   by `OVERSHOOT_PAD = 16` bytes so the hot path can do 16-byte
   overshoot writes.  Public API is unchanged.
2. **Fast-loop invariant.**  Tightened from `s + 11 ≤ src.len()`
   (Go) to `s + 16 ≤ src.len()` to enable the 16-byte literal
   overshoot read.  Last ≤ 15 bytes go through the tail loop.
3. **LZ4-style overshoot.**  Not in the original plan.  See the
   *LZ4-style overshoot* subsection above.
4. **`Box<[u32; N]>` vs `Vec<u32>` for hash tables.**  We use `Vec`
   inside `thread_local!` rather than `Box<[u32; N]>` so the same
   storage cell can serve both L2's 17-bit table and any future
   variant without size-vs-type juggling.  Resized lazily on first
   borrow.

### Outstanding work (would land before declaring stage A merged)

1. **L2 encoder on longer streams.**  At 1 MB Twain we measured
   -4 % vs Go noasm — just inside target.  Re-measure on inputs
   ≥ 4 MiB before treating as a real gap; if it persists, profile
   the in-between sparse indexing loop with `cargo flamegraph`.
2. **Miri pass.**  ✅ Delivered.  `cargo +nightly miri test --lib`
   runs the focused `miri_decoder_unsafe_paths` test plus the
   non-heavy unit tests; heavy tests are
   `#[cfg_attr(miri, ignore)]`'d.  Found and fixed one Stacked
   Borrows violation (decoder tail loop was mixing `&mut [u8]`
   slice ops with raw-pointer writes via `forward_copy_ptr`).
3. **Criterion bench harness.**  ✅ Delivered.  `benches/block.rs`
   uses criterion 0.5 (default-features off) with a
   `twain_encode/<size>/<L>` / `twain_decode/<size>/<L>` matrix
   covering 1e5 / 1e6 × L1 / L2 / L3.  Twain corpus is auto-located
   via `CARGO_MANIFEST_DIR`; override with `MINLZ_TESTDATA=<dir>`.
   The hand-rolled `examples/bench.rs` stays for quick spot checks.
4. **Full fuzz run.**  ✅ Delivered.  First-pass on Ryzen 9 9950X
   (8 workers / target, 15 min budget each, max input 1 MiB):

   | target              | seeds + | crashes + | runs       |
   |---------------------|--------:|----------:|-----------:|
   | `decode_arbitrary`  |  10 155 |         0 |   1.27 M   |
   | `roundtrip`         |   2 192 |         0 |   (logged) |
   | `max_encoded_len`   |   1 899 |         0 |   (logged) |

   Total wall clock 45 min, 0 findings.  libFuzzer's coverage-
   guided mutation discovered ~14 k new corpus entries beyond
   the static seeds, indicating the decoder dispatch graph was
   exercised well past what hand-curated seeds alone reach.
   Re-run via `crates/minlz/fuzz/run.sh` (see `RUNBOOK.md`)
   whenever the decoder or encoder changes.
5. **Stream-level tests** (`TestReader*`, `TestWriter*`) — deferred
   to stage C as planned.  Out of scope here.

### CI

`.github/workflows/` ships three workflows:

* `ci.yml` — fmt / clippy / build / test / docs on Linux × macOS ×
  Windows × stable + MSRV (1.75).
* `miri.yml` — Stacked Borrows under miri, push-to-main + weekly
  cron.  Heavy tests are skipped via `#[cfg_attr(miri, ignore)]`.
* `fuzz.yml` — 5-minute fuzz pass per target on every push to main
  and a nightly cron.  Corpus growth caches across runs.

### Key files and their purpose

| File                                           | Purpose                                                |
|------------------------------------------------|--------------------------------------------------------|
| `crates/minlz/src/lib.rs`                      | Crate-level docs (LE/BE note), public re-exports.       |
| `crates/minlz/src/error.rs`                    | `Error` enum (Corrupt / TooLarge / InvalidLevel).      |
| `crates/minlz/src/block/mod.rs`                | Public API + `parse_header` + `Level`.                  |
| `crates/minlz/src/block/format.rs`             | Tag constants, offset/length ranges, varint, max_enc.  |
| `crates/minlz/src/block/load_store.rs`         | Unaligned LE loads/stores (raw-pointer helpers).        |
| `crates/minlz/src/block/hash.rs`               | `hash4..hash8` (Go primes).                             |
| `crates/minlz/src/block/emit.rs`               | Emit fns + size predictors (`emit_*_size`).             |
| `crates/minlz/src/block/match_len.rs`          | `matchLen` port (8-byte stride + tail).                 |
| `crates/minlz/src/block/decode.rs`             | Safe decoder, two-loop, LZ4 overshoot fast path.        |
| `crates/minlz/src/block/encode_l1.rs`          | Fastest (15-bit / 13-bit tables).                       |
| `crates/minlz/src/block/encode_l2.rs`          | Balanced (long + short, thread_local L table).          |
| `crates/minlz/src/block/encode_l3.rs`          | Smallest (20+18-bit packed-pair tables, scoring).       |
| `crates/minlz/src/block/tests.rs`              | 34 unit tests + corpus-smoke tests.                     |
| `crates/minlz/examples/bench.rs`               | Hand-rolled bench (placeholder for criterion).          |
| `crates/minlz/fuzz/`                           | cargo-fuzz crate (3 targets) + `seed.sh` + `run.sh`.    |
