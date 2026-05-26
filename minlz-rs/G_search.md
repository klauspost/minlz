# Stage G — Block Search

> Read `README.md` first. The project-wide rules apply.
> Stages **A–F** must be DONE. Stage G is the largest and most
> intricate stage. Plan budget: ~3× a typical stage.

## Goal

Optional bloom-style search tables that let a reader skip MinLZ
blocks that **definitely do not** contain a given byte pattern,
without decompressing them.

Three sub-features:

1. **Encoder** — when writing a stream, optionally build and emit
   a `SearchTableInfo` chunk (0x44) and a per-block
   `SearchTable` chunk (0x45) for every compressible block.
2. **Decoder** — `BlockSearcher` reads a stream and, for each
   block, decides via the table whether the block can contain
   the pattern. Skipped blocks bypass decompression entirely.
3. **Search index streams** — a stream that contains only search
   tables and "remote block reference" chunks (0x47), describing
   patterns inside *another* stream's data blocks.

Stage G is governed by:

* `SPEC_SEARCH.md` — the wire format and matching algorithm.
* `SEARCH.md` — the user-facing guide.

## Reference

| Subject                                | Go source                          |
|----------------------------------------|------------------------------------|
| Wire format, hash function             | `SPEC_SEARCH.md` §2, §3            |
| User-facing API                        | `SEARCH.md`                        |
| `SearchTableConfig` and helpers        | `search_table.go`                  |
| Per-block table build                  | `search_index.go`                  |
| Search reader (state machine)          | `search_reader.go`                 |
| Compressed search table (0x46) via huff0| `search_compressed.go`           |
| Asm helpers for table build (popcount, etc.)| `search_asm_*.go`             |
| Boundary handling — encoder            | `search_index.go` overlap loop     |
| Boundary handling — decoder            | `search_reader.go:patternCanMatch`, `canBoundaryMatch`, `patternDeferHashes` |
| Lazy previous block                    | `search_reader.go:lazyBlock`       |
| Deferred block decode (B.4 in SPEC)    | `search_reader.go:pendingBlock`, `resolvePending` |
| Search examples (golden tests)         | `search_examples_test.go`          |
| Search-only/index streams              | `search_reader.go` + `SPEC_SEARCH.md §1.1, §2.3` |

## In scope (mandatory)

### Table types

All four:

* Type 1 — no prefix (every position hashed).
* Type 2 — 1–8 byte prefix list.
* Type 3 — 256-bit prefix mask.
* Type 4 — long prefix (1–256 bytes).

### Chunks

* 0x44 — Search Table Info (per stream).
* 0x45 — Block Search Table (per block).
* **0x46 — Compressed Block Search Table (huff0)** — full
  encoder *and* decoder are in scope. See "0x46 / huff0
  scope" below for the implementation plan.

Chunk 0x47 (Remote Block Reference / search-index-only
streams) is **deferred to a follow-up stage** — there is no Go
implementation to port and the spec work belongs in its own
plan. See "0x47 deferral" below.

### Hash function

`hashValue(val: u64, table_size: u8, match_len: u8) -> u32` —
ports `search_table.go:hashValue` exactly, including the
`tableSize >= 16` special case for `matchLen == 2`.

Primes (from `search_table.go:prime*`):

```rust
const PRIME_2: u32 = 40503;
const PRIME_3: u32 = 506_832_829;
const PRIME_4: u32 = 2_654_435_761;
const PRIME_5: u64 = 889_523_592_379;
const PRIME_6: u64 = 227_718_039_650_203;
const PRIME_7: u64 = 58_295_818_150_454_627;
const PRIME_8: u64 = 0xCF1B_BCDC_B7A5_6463;
```

Per-matchLen specialized helpers (`hashValue1`..`hashValue8`) for
the inner loops.

### Reductions

Halve a table by OR-folding upper into lower. Population-based
stopping condition (`maxReducedPopPct`). Port
`unsafe_enabled.go:reduceTable` and the safe fallback
`unsafe_disabled.go` equivalent.

### Encoder (writer integration)

`WriterOptions::search: Option<SearchTableConfig>` — when set,
the writer emits:

* One 0x44 info chunk after the stream identifier and before any
  data block.
* For each compressible data block (chunkType == 0x02), either
  a 0x45 table chunk or a 0x46 compressed-table chunk
  immediately before the data chunk. The choice follows Go's
  rule (`search_compressed.go:appendSearchTableCompressedChunk`):
  emit 0x46 only when it is **strictly smaller** than the
  equivalent 0x45 (or when `forceCompressed` is set); otherwise
  fall back to 0x45. The "popcount band" exclusion at 50 % ±
  `skip_pct_times_100/100` is preserved verbatim.

Encoder-side overlap: for each block, the next block's first
`matchLen - 1` bytes must be available so that boundary
positions are indexed (see SPEC_SEARCH.md §B.1). In single-thread
mode the overlap is `p[blockSize..]`. In MT mode the dispatcher
slices the overlap **before** handing the block to a worker —
match Go's `writer.go:EncodeBuffer` overlap setup.

For uncompressed blocks, no table is emitted — match Go
(`writer.go` skips when `chunkType == chunkTypeUncompressedData`).

The encoder may emit a table with all bits set (too populated)
as `nil` — Go skips emitting; Rust does the same.

### Decoder (BlockSearcher)

```rust
pub struct BlockSearcher<R> { /* private */ }

impl<R: Read> BlockSearcher<R> {
    pub fn new(r: R) -> Self;
    pub fn with_options(r: R, opts: BlockSearchOptions) -> Self;

    pub fn search<F>(&mut self, pattern: &[u8], fn_: F) -> Result<(), Error>
    where F: FnMut(SearchResult<'_>) -> Result<(), Error>;

    pub fn stats(&self) -> &SearchStats;
}
```

`SearchResult` has the same shape as Go's: two block slices
(`prev`, `current`), match offset, stream offset, block start,
and a lazy `prev_block()` accessor.

Behaviors that **must** match Go exactly:

* `BlockSearchBailOnMissing` — `bail` flag returns
  `Error::SearchTablesUnusable` when tables can't answer.
* `BlockSearchInfoCallback` — fires once on parsing 0x44.
* `BlockSearchIgnoreCRC`.
* `BlockSearchCollectStats`.
* `BlockSearchMaxBlockSize`.

### Boundary handling (SPEC_SEARCH.md Appendix B)

Three subtle correctness requirements:

* **B.2 — first-window-only when pattern is long.** If
  `len(pattern) > matchLen`, only the first matchLen-window may
  be checked; later windows might straddle the block boundary.
  Match `search_reader.go:patternCanMatch`.
* **B.3 — prevBlock retention.** When the current block is
  decoded, retain it so the next block can check the boundary
  via the suffix of `prev`. If the current block is skipped,
  retain the *compressed* data lazily (`prevLazy`) so
  `SearchResult.prev_block()` still works.
* **B.4 — deferred decode.** When `canUse && match` and the
  pattern has more windows than table-confirmed, defer decode
  until the *next* block's table arrives; if the next table
  also lacks the absent windows, the deferred block is skipped.

These three combine into a state machine. Port directly from
`search_reader.go:Search`. Document the state diagram in the
Rust source as a doc comment because the code by itself is hard
to follow.

### Search index–only streams / 0x47 — deferred

A "search index stream" replaces every data chunk (0x02/0x03)
with a 0x47 chunk that lists the offset of the corresponding
data block in another stream. The decoder, when given access to
both streams, can resolve the data on demand.

**Status: deferred to a follow-up stage (call it H).**

Reason: a `rg 0x47` over the Go repo finds only the spec text
(`SPEC_SEARCH.md:161+`). There is no Go writer, no Go reader, no
test, and no fuzz target. Porting would mean implementing the
chunk purely from the spec with no oracle to differ-test
against, in the middle of an already-large stage. The risk vs.
value trade is bad.

The stage-G `BlockSearcher` must still **silently skip** 0x47
chunks if any appear in a stream (they are in the skippable
range 0x40-0x7f), and must reject any attempt to encode them via
the public API. A future stage H will define a concrete
spec-first design, prototype with a hand-built fixture suite,
and add encoder + decoder.

### Asm helpers

`search_asm_*` files contain SIMD-accelerated table build /
popcount helpers on amd64 and arm64. For the Rust port:

* `popcount_bytes(&[u8]) -> usize` — `core::arch::x86_64::_popcnt64`
  on x86_64-v2, otherwise a portable byte loop. ARM64 has
  `vcntq_u8`. Stage A's `cfg_if!` pattern applies.
* `reduce_table(&mut [u8], ...) -> u8` — already cfg-gated unsafe
  in stage A's load-store module style.

These are perf optimizations, not correctness gates. Port the
portable scalar versions first; add SIMD when benchmarks show
need.

## 0x46 / huff0 scope

The Compressed Block Search Table (0x46) wraps the bitmap with
a small entropy codec stack:

* The bitmap is split into 1–16 *sub-blocks* (huff0 blocks),
  each `1 << h0_bs` bytes (`h0_bs` ∈ [5, 17]).
* Each sub-block has a *disposition* picked by the encoder
  among five choices:
  * 0–15 — huff0 4×-interleaved (with one of up to 16 tables
    serialized in the same chunk; tables may be shared across
    sub-blocks).
  * 16 — raw (uncompressed).
  * 17 — RLE (one byte repeated).
  * 18 — sparse bit table (gap-encoded set-bit positions).
  * 19–255 — reserved/invalid.

Per the user decision on stage scoping, **full encoder and
decoder are mandatory.** Match the Go test, fuzz, and bench
surface in `search_compressed_test.go` (998 lines).

### What needs to be built

1. **huff0 codec** (~1500–2500 LOC).
   * `Compress4X` (4-stream interleaved encode) and
     `Decompress4X` matching the wire format in
     [RFC 8878 §4.2](https://datatracker.ietf.org/doc/html/rfc8878#section-4.2).
   * Table build, table serialization, table reuse.
   * Error types: `ErrIncompressible`, `ErrUseRLE` (the two
     error codes Go's `huff0.Compress4X` returns, see
     `search_compressed.go:367`).

   **Implementation choices (pick at the start of stage G):**
   * **G.A — Vendor a clean-room Rust huff0** (single huff0
     module under `crates/minlz/src/search/huff0/`). Adds ~2500
     LOC plus tests but keeps the crate zero-dep.
   * **G.B — Port klauspost/compress/huff0** verbatim from
     Go. Easier line-by-line review; same number of LOC.
   * **G.C — Add `ruzstd` as a dep and reach into its
     `entropy::huff0` module.** Requires verifying the module
     is `pub` (it has historically been `pub(crate)`); if not,
     fork or wrap. Smaller LOC count, but couples us to
     ruzstd's release cadence.

   **Default decision: G.A** (clean-room Rust huff0 in this
   crate). It keeps the dep tree clean and gives full control
   over panic-safety / memory bounds, which matters for a
   library used in storage paths. Confirm at stage start; if
   the timeline is tight, fall through to G.B.

2. **Sparse bit table codec.** Already pure Go in
   `search_compressed.go:appendSparseBitTable` and
   `decodeSparseBitTable`. Port verbatim (~30 LOC each side).

3. **Sub-block disposition picker.** Follows
   `search_compressed.go:appendSearchTableCompressedChunk` —
   computes the cost of every disposition for every sub-block,
   plus the cost of a global shared table, and picks the
   combination that minimizes total chunk size. Includes:
   * Popcount-band skip (`SkippedBand`).
   * Per-block "own table" vs "global table" decision with
     `cstOwnTableBias` penalty.
   * Final `chunk46 < chunk45` gate (unless `forceCompressed`).

4. **0x46 wire format.** Header is the same as 0x45 (type,
   matchLen, baseSize, prefix, reductions, CRC of bitmap)
   followed by `h0_bs`, `h0_tc`, the serialized tables, then
   per-sub-block `h0_ti` + payload.

5. **Compressed-search stats.** Port `CompressedSearchStats`
   from `search_compressed.go:108-130`, including the field
   names so existing dashboards work after a Go ↔ Rust swap.

### Test/fuzz/bench targets

Coverage **must equal** `search_compressed_test.go`. Port the
full Go test/fuzz/bench surface 1:1:

**Unit tests** (from `search_compressed_test.go`):

| Rust target                                       | Go source                                  |
|---------------------------------------------------|---------------------------------------------|
| `compressed::tests::chunk_roundtrip`              | `TestCompressedChunkRoundtrip` (98)         |
| `compressed::tests::popcount_band`                | `TestCompressedPopcountBand` (144)          |
| `compressed::tests::tiny_bitmap_beats_45`         | `TestCompressedTinyBitmapBeats45` (159)     |
| `compressed::tests::rle_all_zero`                 | `TestCompressedRLEAllZero` (179)            |
| `compressed::tests::rle_all_one`                  | `TestCompressedRLEAllOne` (209)             |
| `compressed::tests::global_single_user_wasted`    | `TestCompressedGlobalSingleUserWasted` (233)|
| `compressed::tests::global_shared`                | `TestCompressedGlobalShared` (273)          |
| `compressed::tests::concurrent_decode`            | `TestCompressedConcurrentDecode` (324)      |
| `compressed::tests::reductions_in_stats`          | `TestCompressedReductionsInStats` (364)     |
| `compressed::tests::sparse_bit_table_roundtrip`   | `TestCompressedSparseBitTableRoundtrip` (381)|
| `compressed::tests::sparse_disposition_selected`  | `TestCompressedSparseDispositionSelected` (413)|
| `compressed::tests::reject_malformed`             | `TestCompressedRejectMalformed` (447)       |
| `compressed::tests::writer_roundtrip`             | `TestCompressedWriterRoundtrip` (507)       |
| `compressed::tests::searcher_stats_fields`        | `TestCompressedSearcherStatsFields` (559)   |
| `compressed::tests::searcher_roundtrip`           | `TestCompressedSearcherRoundtrip` (601)     |
| `compressed::tests::fallback_to_45`               | `TestCompressedFallbackTo45` (649)          |
| `compressed::tests::stats_hook`                   | `TestCompressedStatsHook` (664)             |
| `compressed::tests::reduction_interaction`        | `TestCompressedReductionInteraction` (693)  |
| `compressed::tests::small_input_roundtrip`        | `TestCompressedSmallInputRoundtrip` (704)   |

**Fuzz** (from `search_compressed_test.go`):

| Rust target                                                  | Go source                                  |
|--------------------------------------------------------------|---------------------------------------------|
| `fuzz_targets/compressed_search_roundtrip.rs`                | `FuzzCompressedSearchRoundtrip` (733)       |
| `fuzz_targets/decode_compressed_search_chunk.rs`             | `FuzzDecodeCompressedSearchChunk` (829)     |
| `fuzz_targets/huff0_decompress_arbitrary.rs`                 | (Rust-only — covers huff0 codec independently)|

**Benchmarks** (Criterion):

| Rust target                                                  | Go source                                  |
|--------------------------------------------------------------|---------------------------------------------|
| `benches/compressed_search_encode.rs`                        | `BenchmarkCompressedSearchEncode` (848)     |
| `benches/compressed_search_decode.rs`                        | `BenchmarkCompressedSearchDecode` (872)     |
| `benches/compressed_search_encode_by_path.rs`                | `BenchmarkCompressedSearchEncodeByPath` (903)|
| `benches/compressed_search_vs_uncompressed.rs`               | `BenchmarkCompressedSearchVsUncompressed` (963)|

The huff0 codec itself also needs its own unit-test set
(table build, single-symbol, ErrUseRLE, ErrIncompressible,
table reuse, 4×-interleave correctness). Those tests are
crate-internal (no Go counterpart in this repo — huff0 lives
in `github.com/klauspost/compress/huff0`).

Differential test: build a fixture corpus by running the Go
0x46 encoder on a curated input set, then assert the Rust 0x46
encoder produces a byte-identical chunk for the same `(bitmap,
cfg)` inputs. Where byte-identity is intentionally not
guaranteed (e.g. huff0 packs differently depending on
implementation), assert the *Rust* decoder round-trips the
*Go* encoder's output and vice versa.

### Risk register for 0x46

* huff0 has many edge cases (single-symbol tables, ErrUseRLE
  threshold, table reuse correctness). Allocate at least two
  full weeks for huff0 alone.
* The Go encoder uses goroutines per sub-block
  (`search_compressed.go:382-390`). The Rust port can use the
  stage-E thread pool when available, but the **first cut
  should be single-threaded** to keep the codec hermetic;
  parallelize as a follow-up optimization.
* The CRC of bitmap is computed once and shared between the
  0x45 and 0x46 forms; verify the Rust port computes it once
  and reuses to avoid divergence.

## Out of scope

* Asm for the entire bitmap build loop. Scalar build is fast
  enough; SIMD only for popcount.
* Concurrent search-table build during MT encode. The Go writer
  builds the table in the worker goroutine; Rust can do the
  same — verify in tests.
* Indexing search-table chunks (0x45) into the regular index
  (0x40). Spec recommends it; Go has it under `writer.go`.
  Verify the Rust port maintains parity.

## Public API

`SearchTableConfig::with_compression(opts)` enables 0x46 with
default heuristics. The Rust API mirrors
`SearchTableConfig.WithCompression` in `search_table.go:158`.
Builder options on `CompressedOpts`:

* `with_skip_pct(pct: f64)` — sets the 50 % ± band where
  compression is skipped (default 10 %).
* `force_compressed()` — always emit 0x46 when it parses
  (fixture generation only).
* `with_stats_hook(fn)` — `FnMut(CompressedSearchStats)` callback
  per block.

```rust
pub use crate::search::{
    BlockSearcher, BlockSearchOptions,
    SearchTableConfig, SearchStats, SearchResult,
    CompressedOpts, CompressedSearchStats,
};

#[derive(Clone)]
pub struct SearchTableConfig {
    pub match_len: u8,          // 1..=8, default 6
    pub table_type: TableType,
    pub base_table_size: u8,    // log2, computed at writer init
    pub max_pop_pct: u8,        // default 70
    pub max_reduced_pop_pct: u8,// default 25
}

pub enum TableType {
    NoPrefix,
    BytePrefix([u8; 8]),
    MaskPrefix([u8; 32]),
    LongPrefix(Vec<u8>),
}

impl SearchTableConfig {
    pub fn new() -> Self;
    pub fn with_match_len(self, n: u8) -> Self;
    pub fn with_byte_prefix(self, prefixes: &[u8]) -> Self;
    pub fn with_mask_prefix(self, mask: [u8; 32]) -> Self;
    pub fn with_long_prefix(self, prefix: &[u8]) -> Self;
    pub fn with_max_population(self, pct: u8) -> Self;
    pub fn with_max_reduced_population(self, pct: u8) -> Self;
}
```

Builder mirrors Go's `WithXxx`. Validation in `validate()` and
at table-build time — return `Error::InvalidConfig` early.

## File layout

```
crates/minlz/src/search/
├── mod.rs          # public re-exports, top-level docs
├── config.rs       # SearchTableConfig + builders + validate
├── hash.rs         # hashValue + per-matchLen specializations
├── table.rs        # buildSearchTable, reduce, popcount helpers
├── encode.rs       # appendSearchTableChunk, info chunk emission
├── decode.rs       # parseSearchInfo, parseSearchTable
├── compressed/
│   ├── mod.rs      # 0x46 encode / decode, picker
│   ├── sparse.rs   # sparse bit table codec
│   └── stats.rs    # CompressedSearchStats
├── huff0/          # clean-room huff0 (option G.A); ~2500 LOC
│   ├── mod.rs
│   ├── table.rs    # ctable build + serialize, dtable build
│   ├── compress.rs # Compress4X (4-stream interleaved)
│   └── decompress.rs # Decompress4X
├── searcher.rs     # BlockSearcher state machine
├── boundary.rs     # patternCanMatch, canBoundaryMatch, deferred
├── lazy_block.rs   # lazyBlock + pendingBlock
└── stats.rs        # SearchStats type + Display
```

`searcher.rs` is the heaviest file. Plan for ~800 LOC. Use a
sub-module per state if it becomes unmanageable.

## State machine (Rust target)

The Go searcher uses a flat `switch chunkType` with implicit
state in struct fields. The Rust port should keep the same
flat dispatch but document each "implicit state" with comments
or enum variants:

```rust
enum BlockTableState {
    None,
    Loaded {
        cfg: SearchTableConfig,
        reductions: u8,
        table: Box<[u8]>,
    },
}

enum PrevBlockState {
    None,
    Decoded(Box<[u8]>),                  // s.prevBlock in Go
    Lazy(LazyBlock),                     // s.prevLazy in Go
}

enum PendingState {
    None,
    Pending(PendingBlock),                // s.pending in Go
}

enum DeferredMatch {
    None,
    Pending(DeferredMatchData),           // s.deferred in Go
}
```

Each chunk handler updates these states. The 1:1 mapping with
Go makes review and diff-testing tractable.

## Testing plan

### Unit tests

For every public function, a unit test. Examples:

* `hash_value` agrees with a checked-in fixture table of
  `(matchLen, tableSize, val) -> hash`. Generate from Go.
* `build_search_table` for type 1 produces the expected bitmap
  for a small input — fixture from Go.
* `parse_search_info` and `parse_search_table` round-trip.

### Differential tests

Critical: this is the largest area for hidden bugs.

For every fixture in `crates/minlz/tests/fixtures/streams/`:

* Stream with no search tables — Rust searcher must decode all
  blocks and find expected occurrences of N test patterns.
* Stream with type 1 tables — Rust searcher must skip the
  blocks the Go searcher skips, find the same matches, in the
  same order.
* Stream with type 2/3/4 tables — same.
* `SearchStats` must match Go's stats *exactly* on the same
  input + pattern combo. Population min/max/sum, blocks
  searched, blocks skipped, blocks deferred, etc.

Stats are the strongest acceptance test because they reflect
the internal state machine, not just the surface output.

### Boundary tests

Specifically port `search_test.go`'s boundary scenarios. They
are scattered; grep for `boundary`, `pending`, `defer`,
`forward`. Each must produce identical results in Rust.

### Search-only stream tests

Generate a search-only stream in Go (some flag in `mz` does it,
or hand-write one); Rust must parse and yield the same remote
offsets.

### Fuzz

* `fuzz_targets/search_table_parse.rs` — random bytes for
  `parse_search_table`. Must never panic, never read OOB.
* `fuzz_targets/searcher_arbitrary.rs` — random stream bytes
  to `BlockSearcher::search` with a fixed pattern. Must never
  panic, never read OOB. Output is whatever (Go corpus might
  have valid streams).
* `fuzz_targets/search_roundtrip.rs` — random uncompressed
  bytes + random pattern → encode with search tables → search →
  assert: every actual occurrence is found and no false
  negatives.

### Cross-impl

A small Go program `tests/go-search-verify` that takes a
stream, a pattern, and prints SHA-256 of `(stream_offset,
matched_bytes)` for every match. Rust does the same; SHAs
must agree.

### Bench

* `search_skip_rate_{file}` — measure blocks-skipped /
  blocks-total for typical patterns. Should match Go ±1
  percentage point on the same input.
* `search_throughput_{file}` — bytes per second of stream
  scanned. Goal: within 5 % of Go's `noasm` numbers for
  search.

## Risks

* **State machine complexity.** The Go searcher mixes pending,
  deferred, prev-block, prev-lazy, table-no-match, table-
  unusable into one switch. Port it with **one tested
  property at a time** — write the test, then make it pass.
  Do not try to land the whole searcher in one PR.
* **Population stats divergence.** Floating-point comparison
  between Rust and Go can drift on the same input due to
  different float optimization. Compare with epsilon
  tolerance for floats; expect exact match for integers.
* **`patternCanMatch` heuristics.** The match-vs-skip logic
  has many branches; one wrong branch silently breaks search
  correctness (false negatives, the worst kind). Differential
  fuzz is the only practical defense; budget time for it.
* **Boundary B.4 deferral.** This optimization is correctness-
  critical when type 2/3/4 prefix tables are used (see
  SPEC_SEARCH.md §B.4.3 "Safe Deferral Conditions"). Test
  with prefix tables aggressively.
* **0x46 parser robustness.** The 0x46 wire format has many
  fields (h0_bs, h0_tc, per-block h0_ti, sub-block payloads).
  Each field has bounds that, if violated, must produce
  `Error::Corrupt` and not panic / not read OOB. Two test sets
  drive this:
  * `compressed::tests::reject_malformed`
    (`TestCompressedRejectMalformed` at line 447) for hand-
    crafted invalid chunks.
  * `fuzz_targets/decode_compressed_search_chunk.rs` for random
    bytes — every input must terminate without panic, with
    either a valid decoded bitmap or `Error::Corrupt`.
  Both are mandatory before the stage is DONE.

## Success criteria (DONE = all of these)

1. All ported `search_*_test.go` tests pass.
2. Search-with-tables vs search-without-tables produces
   identical results on every test fixture.
3. `SearchStats` matches Go's stats exactly for integers and
   within ±0.5 % for floats on the cross-impl fixture set.
4. Fuzz targets run ≥ 2 hours total without findings.
5. Boundary scenarios (B.2, B.3, B.4, lazy prev) all have a
   dedicated unit test.
6. Search bench is within 5 % of Go `noasm`.
7. 0x46 encode + decode parity with Go: differential test
   round-trips every fixture, huff0 codec passes its own unit
   tests, fuzz target finds no decoder panics in ≥ 1 hour.
8. The Writer correctly emits 0x44 once + 0x45 (or 0x46 when
   smaller) per compressible block, with correct overlap
   behavior at block boundaries.
9. 0x47 Remote Block Reference chunks are silently skipped by
   the decoder (skippable range) and rejected on encode — the
   feature itself is deferred to follow-up stage H.

## Open questions

1. **huff0 (0x46) implementation choice.** Decided: full
   encode + decode is **mandatory**. Default plan is G.A
   (clean-room Rust huff0 in-crate); G.B (port klauspost/compress
   verbatim) is the fallback if the line count or test surface
   blows up. G.C (ruzstd dep) only if the other two prove
   infeasible.
2. **Long prefix length encoding.** Spec says "first byte
   defines prefix length, one must be added". Verify Go reads
   `payload[0] + 1` — yes, see `search_table.go:331`. Port
   exactly; off-by-one here is silent corruption.
3. **Type 2 padding semantics.** Spec says "rest can be filled
   with duplicates of previous ones". Go does this in
   `WithBytePrefix`. Match exactly.
4. **`patternCanMatch` returns `(canUse, match)` — the
   `canUse` is true if any window can be checked. Verify the
   Rust port handles "no windows checked" (canUse=false)
   identically to Go, including the `bail` flag interaction.
5. **Threading.** The searcher is single-threaded today
   (Go too). Parallelizing it is hard because of the
   sequential state machine (prev block, deferred block, etc).
   Out of scope. Document.
6. **Search-table pool reuse.** Go uses `sync.Pool` for table
   buffers. Rust analog is `thread_local!` — same approach as
   stage A's hash tables.
7. **SearchResult lifetime.** Go's `Blocks [2][]byte` are
   "invalid after the callback returns". Rust expresses this
   naturally with `SearchResult<'_>` carrying borrowed slices.
   Document loudly: do not retain `SearchResult` outside the
   callback.
8. **CLI integration.** `mz search` is in `cmd/mz/search.go`
   but explicitly excluded from stage D. After stage G is
   DONE we can add `mz search` as a separate small task; it
   is not a stage G success criterion.
