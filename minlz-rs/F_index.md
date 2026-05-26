# Stage F — Stream Index: Write, Read, Seek

> Read `README.md` first. The project-wide rules apply.
> Stages **A–E** must be DONE. Stage F adds the optional index
> chunk (0x40) and seekable random-access reads.

## Goal

* Build an index of `(compressed_offset, uncompressed_offset)`
  pairs while writing a stream, append it to the end of the
  stream on `finish()`.
* Load an index from the end of an existing stream (search
  backwards for the trailer).
* Seek to an arbitrary uncompressed offset in a seekable source
  using the index.
* Standalone `IndexStream` builder that scans a stream and
  produces an index without touching block data.
* Optional `RemoveIndexHeaders` / `RestoreIndexHeaders` for the
  20-byte-compact form.

## Reference

* `index.go` — entire file (636 lines).
* `SPEC.md` §4.12 — wire format for the index chunk.
* `reader.go` — methods that consume the index for seeking:
  search for `r.index`, `Skip`, `Seek` in the file.
* `writer.go:closeIndex` (line ~894) — how the writer appends the
  index on close.

## Wire format (SPEC.md §4.12)

| Field                  | Encoding                                |
|------------------------|------------------------------------------|
| `id`                   | 1 byte, `0x40`                           |
| `chunk length`         | 3 bytes, LE, of the chunk body          |
| `header`               | 6 bytes, `"s2idx\x00"`                  |
| `total uncompressed`   | varint (zigzag signed)                  |
| `total compressed`     | varint (zigzag signed) (-1 if unknown)  |
| `est block size`       | varint (zigzag, ≥ 0)                    |
| `entries`              | varint (≥ 0, < 65536)                   |
| `has uncompressed offsets` | 1 byte, 0 or 1                      |
| `uncompressed offsets` | `[entries]` varint, decoded via running delta + est-block-size |
| `compressed offsets`   | `[entries]` varint, decoded via running delta + running prediction (half-error) |
| `size`                 | 4 bytes, LE, total chunk size (incl. header + trailer + this) |
| `trailer`              | 6 bytes, `"\x00xdi2s"`                  |

The two delta-encoding schemes (`SPEC.md` §4.12, "Decoding entries:")
are subtle. Port the reference loops verbatim — they are
~30 lines each in `index.go:appendTo` / `Index.Load`.

The legacy S2 index ID is `0x99`; MinLZ uses `0x40`. Both must be
**rejected** at the chunk-type level except for `Index::Load`
which accepts either (Go: `index.go:Load` line ~277).

## In scope

### Types

```rust
pub struct Index {
    pub total_uncompressed: i64,
    pub total_compressed: i64,         // -1 if unknown
    pub offsets: Vec<OffsetPair>,
    pub est_block_uncomp: i64,
}

pub struct OffsetPair {
    pub compressed: i64,
    pub uncompressed: i64,
}
```

`Offsets` is sorted ascending by both keys (invariant).

### Methods

| Rust                                                | Go                                |
|-----------------------------------------------------|-----------------------------------|
| `Index::default()`                                  | zero-value Index                  |
| `Index::reset(&mut self, max_block: usize)`         | `Index.reset`                     |
| `Index::add(&mut self, comp: i64, uncomp: i64) -> Result<()>` | `Index.add`             |
| `Index::find(&self, offset: i64) -> Result<(comp, uncomp)>`   | `Index.Find`            |
| `Index::append_to(&self, buf: &mut Vec<u8>, total_uncomp: i64, total_comp: i64)` | `Index.appendTo` |
| `Index::load(&mut self, buf: &[u8]) -> Result<&[u8]>` | `Index.Load`                     |
| `Index::load_stream<R: Read + Seek>(&mut self, rs: &mut R)` | `Index.LoadStream`         |
| `Index::json(&self) -> String`                      | `Index.JSON`                      |
| `fn index_stream<R: Read>(r: R) -> Result<Vec<u8>>` | `IndexStream`                     |
| `fn remove_index_headers(b: &[u8]) -> Option<&[u8]>`| `RemoveIndexHeaders`              |
| `fn restore_index_headers(b: &[u8]) -> Vec<u8>`     | `RestoreIndexHeaders`             |

`add` enforces:
* New uncompressed offset > previous.
* New compressed offset > previous.
* Don't add a new pair unless the uncompressed gap > `est_block_uncomp`
  (matches Go's "Don't add until we have i.estBlockUncomp").
* If `len(offsets) > 65535` after add, call `reduce_light` to
  prune.

### Writer integration

`Writer::with_options` opts in to index generation
(`WriterOptions { generate_index: true, .. }`, default `true`).
The writer accumulates `(written, uncomp_written)` pairs in its
Index field as blocks are emitted.

`finish()` semantics:

* If `generate_index && !append_index`: index is built but not
  written; `Writer::close_index()` (separate method) consumes the
  Writer and returns `(Vec<u8>, W)` — index bytes and inner writer.
* If `generate_index && append_index`: index is appended to the
  stream after EOF (and any padding).
* If `!generate_index`: nothing extra written.

Padding interaction (`writer.go:closeIndex` lines 919–948):

* Padding is emitted **before** the index, so the *un-padded*
  total size sits at the index-trailer offset.
* If `pad > 1`, `Index.TotalCompressed` is stored as `-1`
  (unknown) — the post-padding total isn't useful for decoder
  seek anyway, since the decoder reads the index from the end.

Match these rules exactly.

### Reader integration

Stage F adds **two** layered APIs onto the stage-C `Reader<R>`:

1. **`Reader<R>` direct methods** — for streaming forward-only
   reads with an already-loaded index. Match Go's
   `reader.go:Skip` and the implicit `Find` lookups.

   ```rust
   impl<R: Read> Reader<R> {
       pub fn skip(&mut self, n: u64) -> io::Result<()>;
       pub fn current_offset(&self) -> u64;   // block_start + r.i
   }
   ```

2. **`ReadSeeker<R>` wrapper** — for seekable inputs and
   random access. Direct port of `reader.go:1304-1491`'s
   `type ReadSeeker struct { *Reader; seek io.Seeker; ... }`.

   ```rust
   pub struct ReadSeeker<R: Read + Seek> {
       reader: Reader<R>,             // wraps stage-C Reader
   }

   impl<R: Read + Seek> ReadSeeker<R> {
       /// Wrap a Reader, optionally taking an externally supplied
       /// index. If `index` is empty, the index is loaded from
       /// the end of the stream via `Index::load_stream`.
       pub fn new(reader: Reader<R>, index: &[u8]) -> Result<Self, SeekError>;

       /// Random-access read. Takes `&mut self` because the
       /// underlying `Reader` advances its decode state on every
       /// read. Side-effect: leaves the Reader positioned at
       /// offset+n (matches Go's documented behavior — see
       /// `reader.go:1317-1318`).
       pub fn read_at(&mut self, p: &mut [u8], offset: u64) -> io::Result<usize>;

       /// Seek by uncompressed offset, returns absolute offset.
       /// Supports SeekFrom::Start, ::Current, and ::End. ::End
       /// requires total_uncompressed != -1.
       pub fn seek_uncompressed(&mut self, pos: SeekFrom) -> io::Result<u64>;

       /// Index access (for tooling, JSON dump, etc.).
       pub fn index(&self) -> &Index;
   }

   impl<R: Read + Seek> Read for ReadSeeker<R> {
       fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
           self.reader.read(p)
       }
   }
   impl<R: Read + Seek> Seek for ReadSeeker<R> {
       fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
           self.seek_uncompressed(pos)
       }
   }
   ```

   `read_at` matches Go's `ReadSeeker.ReadAt` semantics
   except for one deliberate Rust-idiom departure:
   * Returns `< len(p)` only at EOF (with a non-nil error) — same
     as Go.
   * Takes `&mut self` (Go uses `sync.Mutex` to make `ReadAt`
     callable on a shared `*ReadSeeker`). Rust's borrow checker
     gives us the same correctness guarantee for free, but loses
     the "parallel ReadAt on the same ReadSeeker" capability that
     Go's mutex pretends to support (and that Go's own docs
     "do not recommend"). Callers that need parallel random
     access open multiple `ReadSeeker`s over independent file
     handles — same pattern they would use for any other Rust
     `Read + Seek` source.
   * Affects the underlying Reader's position (Go semantics —
     "using Read after ReadAt will continue from where the
     ReadAt stopped").

   No `Mutex<()>` field exists. If a future stage wants the
   "shared `&self` with serialization" Go API, it would need to
   wrap the whole ReadSeeker in interior mutability
   (e.g. `Mutex<ReadSeeker<R>>` exposed publicly) and is not
   in scope here.

   `seek_uncompressed` ports `reader.go:Seek` (line 1373):
   * `SeekFrom::Start(off)` → absolute uncompressed offset.
   * `SeekFrom::Current(d)` → `block_start + r.i + d`.
   * `SeekFrom::End(d)` → `index.total_uncompressed + d`, requires
     `total_uncompressed != -1`.
   * Fast path when `absolute` is inside the current decoded
     block: just adjust `r.i`. Otherwise: `index.find(absolute)`,
     seek the underlying reader, reset Reader state, and `skip`
     any remainder inside the new block.

The legacy single-method form from earlier in this doc —
`Reader::seek_uncompressed` — is **dropped**. Seeking now lives
on `ReadSeeker<R>`, which makes the `R: Read + Seek` constraint
explicit at the type level and matches Go's two-type layout.

### External index bytes

`ReadSeeker::new(reader, index)` accepts an externally-supplied
index payload (the same byte format as the 0x40 chunk including
header and trailer, *or* the bare form produced by
`remove_index_headers`). When the slice is non-empty, the index
is parsed via `Index::load` and the trailer-search step is
skipped — matches `reader.go:1322-1331`.

Use cases (port from Go docs):

* The index is stored separately from the stream (e.g. in a
  sidecar file or DB).
* The caller wants to avoid the seek-to-end probe (cheaper for
  HTTP range readers).
* The stream has no appended index but the caller built one via
  `index_stream(r)`.

`seek_uncompressed`:

1. Lazily load the index via `Index::load_stream(reader)` on
   first call.
2. Use `Index::find(offset)` to get `(comp_off, uncomp_off)`.
3. `inner.seek(SeekFrom::Start(comp_off))`.
4. Reset the chunk dispatch state; set `block_start = uncomp_off`.
5. Set "skip N bytes from next decode" = `offset - uncomp_off`.
6. Next `read()` decodes one block, discards the first
   `offset - uncomp_off` bytes, then returns the rest.

This is identical to Go's `reader.go:Seek` logic — find the file
and grep for `r.index.Find`.

`skip(n)` is the streaming-only sibling: works without `Seek` on
the underlying reader; simply discards `n` decoded bytes by
reading and dropping. Used when index is not available.

`ErrCantSeek` (Go) → a Rust `enum SeekError` mapped into
`io::Error` via `From`. Match the same reasons Go reports
(`reader.go:30-37`):

* Reading the supplied index failed.
* Underlying reader does not implement `Seek` (in Rust, this is
  a compile-time bound, so the case becomes "stream's trailer
  could not be located" — see `LoadStream`).
* Stream has no index appended.

Snappy/S2 fallback is out of scope — skip that case.

## Out of scope

* Snappy/S2 legacy index ID 0x99 — accept on **load** (so we can
  read older streams) but never **emit**. Match Go.
* Concurrent index building during multi-threaded encode — the
  Writer is already single-thread for its `index.add` calls
  (the writer thread does it; workers do not touch `index`).
  No new code, just verify in tests.
* Index seek across concatenated streams (multiple stream
  identifiers in one file). Reject for now; document.

## Algorithm details

### `Index::add` deduplication

When `uncomp_delta < est_block_uncomp`, skip. After every add,
if `len > 65535`, call `reduce_light` which doubles
`est_block_uncomp` and re-thins entries. Port both `reduce` and
`reduce_light` from `index.go:147–185`.

### Compressed offset prediction

`append_to` encodes each compressed offset as a delta from
`previous + running_prediction`. The running prediction starts at
`est_block / 2` and is adjusted by `delta / 2` after each entry.
Port `index.go:241-253` exactly. Same loop in `Load`
(`index.go:373-397`).

### Trailer search in `LoadStream`

Seek to `-10` from end, read 10 bytes:
* Last 6 = trailer (`\x00xdi2s`).
* Preceding 4 = LE u32 total chunk size including header + trailer.
Then seek to `-size` from end, read `size` bytes, call `Load`.

This is the only place where we need `Seek`. Document that
`Reader` without `Seek` cannot use the index.

## File layout

```
crates/minlz/src/index.rs            # ~600 lines, mirrors index.go
crates/minlz/src/stream/seek.rs      # ReadSeeker<R>, ReadAt, SeekError
```

The Index is its own module because it is self-contained (no
dependency on the block codec) and useful standalone — e.g. for
tooling that wants to read an index without decoding any data.

## Testing plan

### Ported tests

`index_test.go` is ~580 lines. Port every `TestIndex*` function:

* Round-trip of empty/small/large indexes.
* Delta encoding correctness.
* `reduce` and `reduce_light` boundary cases.
* `Load` reject on bad headers, bad trailers, truncated data.
* `LoadStream` with offsets that match Go's expectations.
* `IndexStream` producing an index that matches a writer-emitted
  one for the same input.

### Cross-impl

Generate index-bearing streams in Go (`mz c -index ...`) for a
half-dozen inputs. Rust must:
* Read and parse the index from the stream tail.
* Read and parse the same index passed as `external_index_bytes`
  (extract via `Index::append_to`, feed through
  `RemoveIndexHeaders` / `RestoreIndexHeaders`, both forms must
  parse identically).
* Seek to mid-stream offsets via `seek_uncompressed`, read
  forward, match expected bytes.
* `read_at` random reads: produce the same bytes as decoding
  the whole stream and slicing.
* Sequential `read_at` calls leave the Reader positioned at
  `offset + n` (Go semantics) — assert via a follow-up `Read`.

Reverse: Rust-encoded streams with appended index must be
seekable by Go's `mz d` and by Go's `Reader.ReadSeeker`.

### Fuzz

`fuzz_targets/index_load.rs`:
* Random bytes → `Index::load`. Must error or succeed, never
  panic.
* Random index → `append_to` → `load` → must round-trip.

### Bench

Marginal benefit, but include `index_seek_random` (seek to 1000
random offsets in a 1-GiB stream and read 4 KiB at each — matches
the typical analytical-query workload).

## Success criteria (DONE = all of these)

1. All ported `TestIndex*` tests pass.
2. Cross-impl round-trip with Go index encoder/decoder works.
3. Seek-to-offset reads byte-exact correct data.
4. Fuzz `index_load` runs ≥ 30 min without findings.
5. Writer integration: `Writer { append_index: true }` produces
   a stream Go's reader can seek in.
6. `Index::find` returns the right entry on a 65535-entry index
   in O(log n) (test with `cargo bench` to assert).

## Open questions

1. **Should `Index` be `Clone`?** Yes — it's a value type, small
   enough (≤ ~1 MB even at max entries × 16 B each).
2. **`Find` for negative offsets** (distance from end). Go
   supports `offset < 0`. Match it; document that it requires
   `TotalUncompressed != -1`.
3. **Should the public API expose `est_block_uncomp`?** Useful
   for tools; expose as a getter only (no setter).
4. **JSON output.** Go uses `encoding/json` with a tagged shadow
   struct. Rust without `serde`: format by hand (the format is
   small — ~10 lines). Avoid adding `serde` for one method.
5. **Padding rule corner case.** Go writes the index header chunk
   *before* padding when `appendIndex=false` and *after* when
   `true`. Verify this matches `writer.go:closeIndex` and port
   exactly — the ordering matters for round-trip parity.
6. **Truncated stream + LoadStream.** What if the file is
   shorter than 10 bytes? Go returns
   `io.ErrUnexpectedEOF`. Mirror that.
7. **`ReadSeeker::read_at` signature.** Rust uses `&mut self`
   because `Read` and `Seek` both require mutable access; the
   borrow checker enforces what Go's `sync.Mutex` enforces at
   runtime. Callers that need parallel random access open
   multiple `ReadSeeker`s over independent file handles —
   the same pattern Go itself recommends. Documented in
   `ReadSeeker::read_at`'s rustdoc.
8. **Should `index_stream(r)` work over `Read` (not
   `Read + Seek`)?** Yes — it walks the stream forward and
   never seeks. Match Go (`IndexStream` at `index.go:455`
   takes an `io.Reader`).
