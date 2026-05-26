# Stage C — Single-threaded Stream Reader and Writer

> Read `README.md` first. The project-wide rules apply.
> Stages **A** (block codec) and **B** (asm decoder) must be DONE.
> Stage C does not parallelize anything — that is stage E. Stage C
> implements every behavior the Go single-threaded path
> (`concurrency == 1`) supports, including padding, EOF validation,
> user-chunk callbacks, and reset/reuse.

## Goal

Stream-level encoding and decoding, single-threaded, end-to-end:

* `minlz::Reader<R: Read>` consumes a MinLZ stream and yields
  decompressed bytes.
* `minlz::Writer<W: Write>` consumes uncompressed bytes and emits
  a MinLZ stream.
* Round-trips byte-exact via Go's stream format.

Round-trip parity with Go is the headline acceptance test: writing
in Rust must produce a stream Go reads, and vice versa.

## Reference

* `reader.go` — entire file except `DecodeConcurrent` (stage E).
* `writer.go` — entire file, but only the `concurrency == 1`
  branches and `writeSync`. Skip the goroutine code paths for now.
* `SPEC.md` §4 (framing format) and §4.1–§4.11.
* `internal/reference/stream.go` if it exists — small clean
  reference.

## In scope

* Stream identifier chunk (0xff) with `MinLz` magic and block-size
  indicator. Match `reader.go:minLzHeader`, `writer.go:makeHeader`.
* Compressed-data chunks (0x02, 0x03) with masked CRC32C. See
  the **CRC mask** subsection below for the exact Rust spelling
  — `minlz.go:crc` relies on Go operator precedence and the
  literal expression must be rewritten for Rust.
* Uncompressed-data chunk (0x01).
* EOF chunk (0x20) carrying optional uvarint total size.
* Padding chunk (0xfe) — write supports it via `WriterPadding`;
  read silently consumes.
* Reader-side dispatch:
  * Reserved unskippable 0x04–0x3f → `ErrUnsupported`.
  * Reserved skippable 0x40–0x7f → silent skip.
  * User skippable 0x80–0xbf → silent skip, or call back if
    registered.
  * User non-skippable 0xc0–0xfd → error if no callback, else
    callback.
* `ReaderIgnoreStreamIdentifier`, `ReaderIgnoreCRC`,
  `ReaderMaxBlockSize`, `ReaderUserChunkCB` analogs.
* `WriterBlockSize`, `WriterLevel`, `WriterUncompressed`,
  `WriterPadding`, `WriterFlushOnWrite`, `WriterAddUserChunk`.
* Reset/reuse: Writer and Reader can be reset to a new sink/source
  without reallocating internal buffers.

## Out of scope

* All Snappy / S2 fallback. `magicBodyS2`, `magicBodySnappy`,
  `magicChunkS2`, `magicChunkSnappy`, `chunkTypeLegacyCompressedData`,
  `Reader.allowFallback`, `Reader.snappyFrame` — **all removed**.
* Concurrency (the `output chan chan result` machinery in
  `writer.go`). Stage E does that.
* Stream index — `WriterAddIndex` and `Reader.Skip`/`Seek` — that
  is stage F.
* Search tables — those are stage G chunks 0x44/0x45/0x46.
* `DecodeConcurrent`, `WriteTo` (parallel) — stage E.

## CRC mask — Rust spelling and test vector

The Go function (`minlz.go:136`) is:

```go
func crc(b []byte) uint32 {
    c := crc32.Update(0, crcTable, b)
    return c>>15 | c<<17 + 0xa282ead8
}
```

In Go, `|` and `+` have the **same** precedence and associate
left-to-right, so the expression evaluates as
`((c>>15) | (c<<17)) + 0xa282ead8`. The OR-vs-add does not
matter for the low 17 bits (which are zero in `c<<17`), but the
**addition is over the whole 32-bit rotated value** and is what
the Snappy/S2 spec defines.

In **Rust**, `+` has higher precedence than `|`, so the literal
translation would be wrong. The Rust port must use explicit
parens and `wrapping_add` (Rust panics on `+` overflow in debug
builds):

```rust
#[inline]
fn mask_crc32c(c: u32) -> u32 {
    ((c >> 15) | (c << 17)).wrapping_add(0xa282_ead8)
}

#[inline]
pub(crate) fn crc(b: &[u8]) -> u32 {
    mask_crc32c(crc32c::checksum(b))
}
```

**Test vector** (must round-trip through both Go and Rust):

| `b`                 | `crc32c::checksum(b)` | `crc(b)`     |
|---------------------|------------------------|--------------|
| `b""`               | `0x0000_0000`          | `0xa282_ead8`|
| `b"a"`              | `0xc1d0_4330`          | (compute and hard-code in `tests/crc.rs` from Go) |
| `b"123456789"`      | `0xe306_9283`          | (compute and hard-code) |
| `b"The quick..."`   | `(compute)`            | (compute) |

The exact `crc(b)` values come from running the Go function on
the same inputs. Generate them once with a tiny Go helper, paste
into `crates/minlz/tests/crc.rs`, and assert. Do not skip this
test — an off-by-one in the rotate is silent and corrupts every
stream.

### General Go-to-Rust operator-precedence pitfall

The CRC mask is not the only place Go expressions read differently
in Rust.  The stage-A decoder tail loop has several spots like

```go
length = int(uint32(src[s-2]) | uint32(src[s-1])<<8 + 30)
```

In Go this parses as `(src[s-2] | (src[s-1]<<8)) + 30` because `|`
and `+` share precedence and associate left-to-right.  In Rust, `+`
binds tighter than `|`, so the literal translation is *wrong* — you
get `src[s-2] | ((src[s-1]<<8) + 30)`.  Audit every such expression
when porting; parenthesise generously.  Three load-bearing
occurrences in `decode.rs` alone, all of which were caught by the
stage-A unit tests (`TestEmitCopy` and the corpus-smoke
roundtrip).

## Format constants the port must preserve

(All from `minlz.go`.)

| Constant                             | Value         |
|--------------------------------------|---------------|
| `MaxBlockSize`                       | `8 << 20`     |
| `minBlockSize`                       | `4 << 10`     |
| `defaultBlockSize`                   | `2 << 20`     |
| `maxBlockLog`                        | 23            |
| `MaxUserChunkSize`                   | `(1<<24) - 1` |
| `MinUserSkippableChunk`              | `0x80`        |
| `MaxUserSkippableChunk`              | `0xbf`        |
| `MinUserNonSkippableChunk`           | `0xc0`        |
| `MaxUserNonSkippableChunk`           | `0xfd`        |
| `ChunkTypePadding`                   | `0xfe`        |
| `ChunkTypeStreamIdentifier`          | `0xff`        |
| `chunkTypeEOF`                       | `0x20`        |
| `chunkTypeUncompressedData`          | `0x01`        |
| `chunkTypeMinLZCompressedData`       | `0x02`        |
| `chunkTypeMinLZCompressedDataCompCRC`| `0x03`        |

## API design

### Reader

```rust
pub struct Reader<R> { /* private */ }

impl<R: Read> Reader<R> {
    pub fn new(r: R) -> Self;
    pub fn with_options(r: R, opts: ReaderOptions) -> Self;

    pub fn reset(&mut self, r: R);
    pub fn block_start(&self) -> u64;  // uncompressed offset
}

impl<R: Read> Read for Reader<R> { /* the dispatch loop */ }
```

`ReaderOptions` is a builder-style struct:

```rust
#[derive(Default, Clone)]
pub struct ReaderOptions {
    pub max_block_size: Option<usize>,
    pub ignore_stream_id: bool,
    pub ignore_crc: bool,
    pub user_chunk_cb: Option<Vec<(u8, Box<dyn FnMut(&mut dyn Read) -> Result<(), Error>>)>>,
}
```

Callbacks: Go uses an array indexed by `id - MinUserSkippableChunk`.
Rust uses a `HashMap<u8, ...>` or a fixed `[Option<Box<dyn ...>>; 126]`
array. Use the array — it's small and avoids hashing on every
chunk.

Errors are returned as a new `minlz::stream::Error` enum (separate
from block-level `Error`):

```rust
pub enum StreamError {
    Block(block::Error),       // bubbles from decoder
    Corrupt,                   // ErrCorrupt
    Crc,                       // ErrCRC
    TooLarge,                  // ErrTooLarge
    Unsupported,               // ErrUnsupported
    Io(std::io::Error),
}

impl From<StreamError> for std::io::Error { ... }
```

`Reader::read` returns `io::Result<usize>` so existing `Read`
combinators work. `From<StreamError> for io::Error` converts using
`io::ErrorKind::InvalidData` for everything except `Io(e)` which
unwraps to `e`.

### Writer

```rust
pub struct Writer<W> { /* private */ }

impl<W: Write> Writer<W> {
    pub fn new(w: W) -> Self;
    pub fn with_options(w: W, opts: WriterOptions) -> Self;

    pub fn reset(&mut self, w: W);
    pub fn flush_async(&mut self) -> io::Result<()>;
    pub fn finish(self) -> io::Result<W>;   // drop sentinel + EOF
    pub fn add_user_chunk(&mut self, id: u8, data: &[u8]) -> io::Result<()>;
    pub fn written(&self) -> (u64 /* input */, u64 /* output */);
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, p: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;  // does *not* emit EOF
}
```

`finish` is the explicit close — Go's `Close()`. Drop **must not**
silently emit EOF; emit a `log::warn!` (only if a `log` feature is
on — *no* hard dep on the `log` crate) and leak the close. Rust
convention: encoders expose `finish()` and require the caller to
call it; flate2's `GzEncoder` is the canonical example.

`WriterOptions`:

```rust
#[derive(Clone)]
pub struct WriterOptions {
    pub block_size: usize,       // default = defaultBlockSize (2 MiB)
    pub level: Level,            // default = Balanced
    pub uncompressed: bool,      // overrides level if true
    pub padding: u32,            // 0 or 1 => no pad, else multiple
    pub flush_on_write: bool,    // FlushOnWrite analog
}
```

`Default::default()` matches Go's `NewWriter` defaults
(`level = Balanced`, `block_size = defaultBlockSize`, no padding,
no flush-on-write).

### Reader/Writer state machines

These must be **deterministic** and match Go behavior bit-for-bit
on equivalent inputs. Where Go uses a mutable struct field as part
of its state machine, mirror it with a single `enum ReadState`
inside the Reader — easier to reason about.

`ReadState`:

* `Init` — expecting stream identifier (unless `ignore_stream_id`)
* `Ready` — between chunks
* `InEof` — saw EOF; will accept either end-of-stream or a new
  stream identifier for concatenated streams (`SPEC.md` §4.1: "To
  allow concatenation, a stream identifier can follow an EOF chunk")
* `Errored` — sticky error

Each chunk dispatch reads 4 bytes (chunk type + 3-byte length)
and matches by type. Compressed chunks decompress into an internal
`decoded: Vec<u8>` buffer (re-used). The `Read` impl drains
`decoded[pos..]` first, then reads more chunks.

### Buffer management

Go uses two buffers per writer (`ibuf`, plus `sync.Pool` of obufs).
Stage C needs only one input buffer and one output buffer (single-
threaded). Concrete sizing:

* `ibuf: Vec<u8>` with `cap = block_size`.
* `obuf: Vec<u8>` with `cap = block_size + obuf_header_len + max_overhead`.

The Reader maintains:

* `buf: Vec<u8>` for raw chunk reads (sized by `max_block_size +
  checksum_size + overhead`).
* `decoded: Vec<u8>` for the current block's decompressed output.

Both grow lazily up to `max_block_size`. The `max_block_size`
comes from the stream identifier's block-size indicator (`SPEC.md`
§4.1.1); the Reader option overrides it (`reader.go:ReaderMaxBlockSize`).

## Round-trip + cross-impl test plan

### Round-trip

In `crates/minlz/tests/stream_roundtrip.rs`:

* For each level in `{Fastest, Balanced, Smallest}` and a
  representative set of block sizes (4 KiB, 64 KiB, 1 MiB, 8 MiB):
  * Encode `testdata` files into an in-memory `Vec<u8>`.
  * Decode back. Assert byte-equality and that
    `Reader::block_start()` advances to `src.len()`.
* Run `flush_on_write` mode — every `write` should emit a chunk;
  the resulting stream should still round-trip.
* `add_user_chunk(0x80..=0xfd, payload)` — encoder must emit the
  chunk; decoder, with a callback, must invoke it with the same
  payload.

### Cross-implementation (Go ↔ Rust)

Checked-in fixtures under `crates/minlz/tests/fixtures/streams/`:

* For 6 inputs × 3 levels × 2 block sizes × {pad=0,pad=512} = 72
  Go-generated `.mz` files. Rust must decode each to its original.
* For 6 inputs, Rust-encode at each level, then verify the file
  decodes correctly when piped through the Go `mz` binary in
  the CI step (or via a pre-recorded `go-decode.sh` fixture that
  shipped alongside).

Reverse direction (Rust-encoded → Go-decoded) is tested via a
small Go program under `crates/minlz/tests/go-verify/` whose output
is captured in CI. The Go program reads `.mz` from stdin and
prints SHA-256 of the decoded bytes; Rust tests compare against
the expected SHA.

### Padding / EOF tests

* Stream with explicit padding (`padding = 4096`): output size
  must be a multiple of 4096 (port of
  `writer_test.go:TestWriterPadding`).
* Stream with no EOF (`ignore_stream_id` not set): reader returns
  `Corrupt` on EOF without seeing chunk 0x20 — port of Go's
  `TestReaderEOFValidation` (search for the exact name).
* Stream with extra bytes after EOF: error.

### Reader options

| Test                                | Go reference                                  |
|-------------------------------------|-----------------------------------------------|
| ignore_stream_id allows mid-stream  | `reader.go:ReaderIgnoreStreamIdentifier`      |
| ignore_crc skips CRC validation     | `reader.go:ReaderIgnoreCRC`                   |
| max_block_size rejects bigger blocks| `reader.go:ReaderMaxBlockSize`                |
| user_chunk_cb fires for skippable   | `reader.go:ReaderUserChunkCB`                 |
| user_chunk_cb error aborts decode   | same                                          |
| concatenated streams                | `reader.go:Read` switch on `ChunkTypeStreamIdentifier`|

### Fuzz

A new fuzz target `stream_decode_arbitrary.rs`:

* Feed random bytes to `Reader<&[u8]>::read_to_end(&mut Vec)`.
* Reader must return either `Ok` or `Err`, never panic, never
  read past `src`, never write more than `max_block_size`.
* Seed corpus: collect from `testdata/fuzz/` if a stream-level
  corpus exists; otherwise generate fixtures via the Go
  `FuzzStream*` corpus.

A second fuzz target `stream_roundtrip.rs`:

* Random input → Rust Writer → Rust Reader → assert equality.

### Bench

Criterion in `crates/minlz/benches/stream.rs`:

* `stream_encode_{file}`, `stream_decode_{file}`, both at Balanced
  level. Targets within 5 % of Go's stream-mode `noasm` benchmark
  on the same machine.

## Mapping table (Go → Rust)

| Go                                | Rust                                                            |
|-----------------------------------|-----------------------------------------------------------------|
| `NewReader(r, opts...)`           | `Reader::with_options(r, opts)`                                 |
| `Reader.Read([]byte) (int, err)`  | `<Reader as Read>::read(&mut [u8]) -> io::Result<usize>`        |
| `Reader.Reset(r)`                 | `Reader::reset(r)`                                              |
| `Reader.GetBufferCapacity()`      | `Reader::buffer_capacity()` (debug helper only)                 |
| `NewWriter(w, opts...)`           | `Writer::with_options(w, opts)`                                 |
| `Writer.Write(p)`                 | `<Writer as Write>::write`                                      |
| `Writer.ReadFrom(r)`              | `Writer::read_from<R: Read>(r) -> io::Result<u64>`              |
| `Writer.AsyncFlush()`             | `Writer::flush_async()` (no-op in single-threaded mode)         |
| `Writer.Flush()`                  | `<Writer as Write>::flush`                                      |
| `Writer.Close()`                  | `Writer::finish()` (consuming)                                  |
| `Writer.AddUserChunk(id, data)`   | `Writer::add_user_chunk`                                        |
| `WriterPadding(n)`                | `WriterOptions { padding: n, .. }`                              |
| `WriterFlushOnWrite()`            | `WriterOptions { flush_on_write: true, .. }`                    |
| `EncodeBuffer(buf)`               | `Writer::encode_buffer(&[u8])` — needs ownership clarity        |

`EncodeBuffer` is the Go "zero-copy" path that requires the buffer
to outlive the `Flush` call. In Rust this is naturally expressed
as `Writer::encode_buffer(&[u8])` where the lifetime ensures the
buffer outlives the call — but only because stage C is
single-threaded. **Document that the stage-E parallel version may
require an owned `Vec<u8>` or a leaked allocation.**

## Risks

* `ReadFrom` in Go reuses the byter Bytes() short-circuit. The
  Rust equivalent should special-case `io::Cursor<Vec<u8>>` and
  `io::Cursor<&[u8]>` via `<R as io::Read>::read_to_end` —
  optional optimization, defer to bench results.
* The `flush_on_write` path emits a chunk per `write()` call —
  trivially slow for many small writes. This matches Go behavior;
  document it.
* The reader's user-chunk callback in Go takes an `io.Reader`
  limited to the chunk length. The Rust equivalent is
  `&mut Take<&mut Reader>` or a custom wrapper. Use `Take` from
  std — it composes cleanly.

## Success criteria (DONE = all of these)

1. All stream tests pass.
2. Fuzz targets run ≥ 1h with no findings.
3. Cross-implementation tests (Rust↔Go) pass for the fixture set.
4. Stream benchmarks within 5 % of Go `noasm` stream-mode numbers.
5. `cargo doc` builds; every public item has rustdoc.
6. No `unsafe` in the stream module (block module hot path may
   keep its `unsafe`).
7. `Drop` for Writer does not eat data silently and does not
   panic; it returns nothing but logs to `log::warn!` if the
   `log` feature is enabled.

## Open questions

1. **`finish()` ergonomics.** Consuming `Writer::finish(self)`
   prevents calling other methods after; that is the safe
   default. Alternative: `&mut self` finish + a `done: bool`
   flag. Default to `self` (consuming).
2. **`flush()` semantics.** Match Go: flushes buffered data but
   does **not** emit EOF. EOF is `finish` only.
3. **`Write::write` partial writes.** Match Go: always returns
   `len(p)` on success. Document this so callers don't loop.
4. **`Reader` peek.** Go allows triggering user-chunk callbacks
   with a 0-byte read. Provide `Reader::peek_chunks() -> io::Result<()>`
   as a more discoverable API. Or: make `read(&mut [])` behave
   the same way. Pick one and document.
5. **CRC implementation.** crc32c with Castagnoli polynomial.
   `crc32fast` crate is widely used but doesn't have CRC32C. Two
   choices:
   * Bundle a small CRC32C implementation (~50 LOC, table-based).
     Optional SIMD via `core::arch` when target supports CLMUL or
     `crc32c` instruction (x86 SSE4.2, arm64 `CRC32CB`/`CRC32CW`).
   * Add `crc32c` crate as a dependency — well-maintained, no
     transitive deps.

   Recommended: bundle for portability; revisit if it shows up in
   profiles. Stage C is allowed to add `crc32c` as the only new
   dep if needed. Whichever route is taken, the `mask_crc32c`
   wrapper and the test vector above are non-negotiable.
6. **Backpressure on writes.** Single-threaded so there's nothing
   to backpressure; this question moves to stage E.
