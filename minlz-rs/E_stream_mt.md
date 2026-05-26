# Stage E — Multi-threaded Stream Reader and Writer

> Read `README.md` first. The project-wide rules apply.
> Stages **A, B, C, D** must be DONE. Stage E adds parallelism on
> top of the single-threaded stream from C; it must not regress the
> single-threaded path.

## Goal

* Parallel **encoder**: blocks are encoded concurrently on a thread
  pool while a single writer thread writes them to the sink in
  input order. `W` is owned by the writer thread for the lifetime
  of the encoder; on `finish()` it is returned to the caller via
  a one-shot channel (see "Ownership of `W`" below).
* Parallel **decoder**: chunks are read from the source, blocks
  are decoded concurrently, and the decoded bytes are emitted to
  the sink (or to the read callback) in stream order.
* Same external API as stage C; concurrency is configured via
  `WriterOptions::concurrency` and a new
  `Reader::decode_concurrent` method.

The throughput target is bounded by the *single-block* throughput
(the stage-A encoder/decoder) — multithreading scales linearly with
core count up to "blocks in flight". Concretely: a stream-encode
benchmark at 8 MiB blocks and 8 threads should hit 7×–8× the
single-thread number on an 8-core machine.

## Reference

* `writer.go` — the `Reset` goroutine pump (line ~165), `write`
  (line ~520), `writeFull`, `EncodeBuffer`, `Flush`, `AsyncFlush`.
  The output channel structure is `chan chan result` — a queue
  of single-slot promises that the writer goroutine drains in
  order.
* `reader.go:DecodeConcurrent` (line ~575) — the parallel
  decoder. Uses three channels: `toRead` (input buffers),
  `writtenBlocks` (decoded output buffers), and `queue` of
  ordered write-permits.
* `reader.go:WriteTo` — convenience wrapper that picks
  `DecodeConcurrent` if it can.

Both Go paths follow the same pattern:

1. The dispatcher (single thread) reads/writes from the source/sink
   and hands each block to a worker via a per-block channel.
2. The worker compresses/decompresses and pushes the result into
   the channel.
3. A single ordering thread drains the channels in submission order
   and writes/emits.

The ordering thread is what guarantees output order despite
worker parallelism. Rust must reproduce this exactly.

## In scope

### Writer

* `WriterOptions::concurrency: NonZeroUsize` (default
  `std::thread::available_parallelism()`). Value of 1 keeps the
  stage-C single-threaded path.
* Background worker pool: scoped threads or a hand-rolled thread
  pool. Workers receive blocks via a bounded channel and produce
  encoded buffers.
* A single writer thread that takes per-block "result futures" in
  submission order and writes their contents to the underlying
  `Write`.
* `Writer::encode_buffer(&[u8])` and `Writer::read_from<R: Read>(r)`
  for high-throughput input.
* `Writer::flush_async()` — does *not* block on output to be
  flushed, but ensures the input buffer is queued.
* `Writer::finish()` waits for all workers and the writer thread,
  then emits EOF (and padding/index/search chunks in later
  stages).

### Reader

* `Reader::decode_concurrent<W: Write>(w: W, threads: usize) -> io::Result<u64>`
  — drains the stream into `w`, parallel-decoded. Cannot be mixed
  with `Read`/`Reset` calls (matches Go's invariant).
* `Reader::write_to<W: Write>(w: W) -> io::Result<u64>` — picks
  concurrent decode if the source isn't already buffered into a
  single block.

The synchronous `Read` impl from stage C stays unchanged.

## Out of scope

* Index reading from concurrent decoder — that's stage F.
* Search-table aware decoder — that's stage G.
* Worker-pool reuse across multiple Reader/Writer instances. Each
  Reader/Writer owns its threads for the duration of the stream.
  A global pool can be added later if profiling justifies it.

## Concurrency primitives

Rust offers several options for the channel + worker pattern.
Picking is a stage-E design decision; pick **one** and stick to it.

### Option A — `std::sync::mpsc` + scoped threads (preferred)

* `std::thread::scope` lets workers borrow Writer state directly,
  no `Arc`. Available since Rust 1.63.
* `std::sync::mpsc::sync_channel(N)` for bounded queues; matches
  Go's `chan chan result` capacity.
* No external deps.

**Catch:** scoped threads tie the worker lifetime to a single
function call. The Writer is a long-lived struct that may
encode many blocks over its lifetime. Solution: each `Writer`
spawns workers in its constructor and holds `JoinHandle`s; on
`finish`, it joins. We can't use `thread::scope` for that —
back to `std::thread::spawn` + `Arc<...>`.

### Option B — `crossbeam-channel` + `std::thread::spawn`

* `crossbeam-channel` is faster than `mpsc` (more flexible
  ordering, MPMC, well-maintained). Single small dep with no
  transitive deps.
* Trade: one extra crate vs. ~20 % better throughput on the
  channel hot path.

### Option C — `rayon`

* Excludes; `rayon` is fork-join and doesn't match the streaming
  in-order writer pattern well. Plus it's a heavy dep.

**Decision: Option A** — `std::sync::mpsc` and `std::thread::spawn`,
with the Writer holding `Vec<JoinHandle<()>>`. If channel overhead
shows up in profiles, revisit Option B.

## Algorithm — Writer

Mirror `writer.go`'s `output chan chan result`:

```
                                  +---- worker 1 ----+
[Writer.write(p)] -> submit block  |   compress       |
                                   +-> result via chan+
                                                       \
                                                        \
                                  +---- worker 2 ----+   \
                                  |   compress       |    +-> writer_thread
                                  +-> result via chan+   /     drains in order
                                                        /
                                                       /
                              ...
```

### Ownership of `W`

`std::thread::spawn` requires `'static + Send` captures. The
naive "store `W` in the struct and write from a background
thread" design will not compile: the writer thread needs
exclusive ownership of `W` while the struct simultaneously
exposes methods that touch it (errors, `finish()`).

Three options that compile cleanly:

**Option E1 — `W` lives in the writer thread; ownership round-trip
on `finish()` (preferred).**

The Writer struct **does not** hold `W` directly. `W` is moved
into the writer thread at construction. To return `W` from
`finish()`, the writer thread sends it back via a one-shot
channel when its input queue closes.

```rust
pub struct Writer<W: Write + Send + 'static> {
    workers: Vec<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
    submit_tx: mpsc::SyncSender<BlockJob>,
    order_tx:  mpsc::SyncSender<mpsc::Receiver<Result<Vec<u8>, StreamError>>>,
    finish_rx: mpsc::Receiver<Result<W, StreamError>>,
    err: Arc<Mutex<Option<StreamError>>>,
    ibuf: Vec<u8>,
    opts: WriterOptions,
    state: WriterState,
}
```

The `W: 'static` bound is real and intentional: the writer thread
outlives any non-`'static` borrow. Most usage (`File`, `TcpStream`,
`Vec<u8>`) satisfies `'static` trivially. Borrowed writers
(`&mut Vec<u8>`) are **not supported in MT mode** — callers must
own their sink or wrap it in a small adapter.

`finish(self) -> io::Result<W>` semantics:

1. Drop `submit_tx` (signals workers to exit after draining).
2. Drop `order_tx` (signals writer thread to exit after draining).
3. Join all workers and the writer thread.
4. `finish_rx.recv()` yields the `W` the writer thread sent back
   just before exiting.
5. Return `Ok(W)` or the sticky error.

The writer thread's loop ends with:

```rust
fn writer_thread_main<W: Write>(
    mut w: W,
    order_rx: mpsc::Receiver<mpsc::Receiver<Result<Vec<u8>, StreamError>>>,
    finish_tx: mpsc::SyncSender<Result<W, StreamError>>,
    err: Arc<Mutex<Option<StreamError>>>,
) {
    let result = (|| -> Result<(), StreamError> {
        while let Ok(rx) = order_rx.recv() {
            match rx.recv() {
                Ok(Ok(buf))  => w.write_all(&buf)?,
                Ok(Err(e))   => return Err(e),
                Err(_)       => return Err(StreamError::WorkerDropped),
            }
        }
        Ok(())
    })();
    let payload = match result {
        Ok(()) => Ok(w),
        Err(e) => { *err.lock().unwrap() = Some(e.clone()); Err(e) }
    };
    let _ = finish_tx.send(payload);
}
```

This is the same lifetime pattern `flate2::GzEncoder` and
`zstd::Encoder` use under the hood for their owned-writer modes.

**Option E2 — `Arc<Mutex<W>>` and lock per write.**

Pop the W into shared state. Workers and (more typically) a
single writer thread acquire the lock to write. `finish` does
`Arc::try_unwrap(arc).expect("…").into_inner().expect("…")` after
joining all threads. Easy to write, but every write goes through
a mutex (no real perf hit since writes are sequential anyway).

Use this only if option E1 forces awkward generics across stage
F/G integration. Default is E1.

**Option E3 — borrowed writer via `std::thread::scope`.**

`thread::scope` lets workers borrow `&mut W`. But scoped threads
require the Writer to live entirely inside a single function
call — incompatible with the long-lived `Writer<W>` API we
inherit from stage C. Reject.

**Decision: Option E1.** All other code in this stage doc assumes
E1. The `W: 'static + Send` bound is mentioned at every API
that exposes it.

### Per-block dispatch

`BlockJob`:

```rust
struct BlockJob {
    uncompressed: Vec<u8>,      // owned by the worker
    start_offset: u64,
    result_tx:    mpsc::SyncSender<Result<Vec<u8>, StreamError>>,
    opts:         WorkerOpts,   // level, search cfg, etc., cheap Clone
}
```

For each block:

1. `Writer::write` (or `encode_buffer`) sets up a per-block
   `(result_tx, result_rx)` of capacity 1.
2. The Writer pushes `result_rx` onto `order_tx` first —
   reserving the block's slot in the output order.
3. It then submits the `BlockJob { result_tx, … }` to a worker
   via `submit_tx`. Worker compresses, sends the encoded buffer
   on `result_tx`, drops the sender.
4. The writer thread (see E1) drains `order_rx`, then drains
   each `result_rx`, then writes to its locally owned `W`.

The pattern is straight from `writer.go:174` "Start a writer
goroutine that will write all output in order." with Rust channel
types and ownership made explicit.

### Buffer ownership

`BlockJob.uncompressed` is owned by the worker. The Writer
copies user-supplied bytes into freshly-`Vec::new`'d (or pool-
reused) buffers before submission. This matches the safety
guarantee of Go's `Write` path.

Go's `EncodeBuffer` requires the caller not to mutate the buffer
until `Flush`. The Rust equivalent in MT mode takes owned data:

* `Writer::encode_buffer(&mut self, buf: Box<[u8]>) -> io::Result<()>`
  — moves the buffer into the job; no copy.

There is **no** `&[u8]` variant in MT mode. The single-threaded
mode (stage C) keeps the borrowed-slice API; MT mode forces
ownership. Document this in the API reference so callers know
which mode they are in.

### Backpressure

`submit_tx` is bounded at `concurrency` (or `concurrency + 1` —
match Go's "extra buffer for read-ahead" comment at
`reader.go:622`). When the queue is full, `Writer::write` blocks.
This is the desired behavior; do **not** buffer unboundedly.

### Error propagation

If a worker fails (e.g. encoder bug), it puts the error into the
`result_tx`. The writer thread, on receiving an error, sets a
shared `Mutex<Option<Error>>` and stops writing. The next user
call to `write`/`flush_async`/`finish` returns that error.

This matches Go's `errMu`-protected `errState`.

## Algorithm — Reader

`Reader::decode_concurrent` is structurally simpler because the
dispatch (reading bytes off the source, chunk header dispatch) is
inherently serial — only the *block decode* can be parallel.

Mirror `reader.go:DecodeConcurrent`:

```
[reader thread]                    +-- decoder 1 --+
   chunk headers   --> per-block ->|  minlz_decode |--+
   read compressed     decode job  +-- decoder 2 --+   --> write_thread
   bytes into buffer               ...                     in stream order
```

* The reader thread reads chunk headers, allocates a buffer from
  a pool, reads the compressed bytes, hands the buffer + a one-
  slot result channel to a worker, and pushes the channel onto
  the FIFO that the writer thread drains.
* Each worker:
  * For 0x02/0x03: decode via `block::decode`, validate CRC, send
    decoded buffer to result_tx.
  * For 0x01: just CRC and forward.
  * Other chunks bypass the worker pool — handled inline by the
    reader thread (stream identifier, EOF, skippable, padding).
* A dedicated **writer thread** owns `W` and drains
  per-block result receivers in submission order, writing each
  decoded buffer to `W` directly.

Why a dedicated writer thread (not Go's "let the worker write"
optimization): sharing `&mut W` across worker threads requires
either `Arc<Mutex<W>>` (defeats the cache-locality win) or
`thread::scope` (incompatible with the long-lived `Reader<R>`
struct). The single-writer-thread design parallels the encoder
(stage E1 "Ownership of `W`") and uses the same handoff channel
to return `W` on completion.

Go's optimization — having the decompression goroutine do the
write — buys 5–10 % via L1-cache locality. We accept that cost
in exchange for code clarity and a sound Rust ownership model.
**Revisit only if benchmarks show > 5 % regression vs. Go.** If
revisited, the path forward is: workers hold `Arc<Mutex<W>>` and
acquire under a "write permit" channel that serializes access —
*not* `&mut W` over a channel, which cannot work with
`thread::spawn`.

### Memory management

A small pool of buffers is recycled:

* `compressed_pool: SegQueue<Vec<u8>>` or a bounded `crossbeam_queue`
  — but without that dep, use `Mutex<Vec<Vec<u8>>>` (it's hit ≤
  once per block, lock contention is negligible).
* `decoded_pool` similarly.

Pool size = `concurrency + 1` (one extra for read-ahead, matching
Go's "extra in+out block" comment at `reader.go:622-624`).

## Public API additions

```rust
impl<W: Write + Send + 'static> Writer<W> {
    // already in stage C, semantics change: now spawns workers
    pub fn with_options(w: W, opts: WriterOptions) -> Self;

    // new in stage E; consumes self and returns W via the writer
    // thread's finish-channel handoff (see "Ownership of W" above)
    pub fn finish(self) -> io::Result<W>;

    // new in stage E
    pub fn encode_buffer(&mut self, buf: Box<[u8]>) -> io::Result<()>;
    pub fn read_from<R: Read>(&mut self, r: R) -> io::Result<u64>;
}

pub struct WriterOptions {
    // ...existing...
    pub concurrency: NonZeroUsize,
}

impl<R: Read> Reader<R> {
    // new — W is moved into a dedicated writer thread for the
    // duration of the call. The Send + 'static bounds are
    // required because std::thread::spawn captures must satisfy
    // them; the alternative (scoped threads via thread::scope)
    // would force decode_concurrent to be a closure-shaped
    // method, which is awkward. Most W in practice (Vec<u8>,
    // File, TcpStream, BufWriter<File>) satisfies these trivially.
    pub fn decode_concurrent<W>(&mut self, w: W, threads: usize) -> io::Result<u64>
    where W: Write + Send + 'static;

    pub fn write_to<W>(&mut self, w: W) -> io::Result<u64>
    where W: Write + Send + 'static;
}
```

`Reader::Read` still works (single-threaded path). Mixing
`Read::read` with `decode_concurrent` returns `InvalidInput` —
match Go's "DecodeConcurrent called after Read" check.

**Borrowed-`W` callers** (e.g. `&mut Vec<u8>` or `&mut File`):
they can wrap their writer in `BufWriter` and pass it by value,
or copy the result into their borrowed sink afterward using
`write_to` on an owned intermediate (e.g. `Vec<u8>`). The
`Send + 'static` constraint cannot be relaxed without dropping
`std::thread::spawn` entirely.

## Testing plan

### Equivalence with stage C

For every test in stage C, add a variant that runs with
`concurrency = 4` (and `decode_concurrent(4)`). Output must be
byte-identical to the concurrency-1 path.

### Stress / race

Use `loom` (dev-dep, gated behind a `loom-test` feature) to model
the channel state machine for ≤ 3 blocks and ≤ 2 workers. Run in
CI to catch ordering bugs.

If `loom` proves too slow or too restrictive, fall back to
`shuttle` (Microsoft's randomized scheduling) or just run the
real-thread test 10,000 times with `cargo nextest`.

### Determinism

Random input → encode at concurrency N → decode at concurrency
M → assert equality, for N, M ∈ {1, 2, 4, 8}. Repeat 100x to
flush out scheduler-dependent bugs.

### Cross-impl

Reuse stage-C fixtures. Add new fixtures encoded with Go at
`concurrency=8` to confirm we can decode them.

### Bench

* `stream_encode_mt_{file}` — at concurrency = available_parallelism.
  Target: ≥ 6× single-thread throughput on an 8-core machine.
* `stream_decode_mt_{file}` — same target.
* On a 1-core machine (e.g. CI macos-arm64 sometimes), targets
  scale linearly with available cores.

## Risks

* The "writer goroutine does the writing" optimization in Go is
  the kind of micro-detail that makes a 5–10 % perf difference.
  Default to the simpler "dedicated writer thread" pattern;
  upgrade if needed.
* Bounded channels can deadlock under panic. Workers must
  `catch_unwind` or the writer must time out / handle disconnect.
  Document and test the "worker panic" path.
* `available_parallelism()` lies on some systems (e.g. cgroups
  limit). Document the env var override `MINLZ_THREADS` if we add
  one.

## Success criteria (DONE = all of these)

1. All stage-C tests pass at concurrency=1, 2, 4, 8.
2. New MT round-trip tests pass with cross-concurrency
   (encode N, decode M).
3. Cross-impl tests pass against Go-MT-encoded streams.
4. MT encode/decode bench shows ≥ 6× single-thread on an
   8-core machine.
5. Loom test for the channel state machine passes.
6. No worker thread leak: `Drop`-ing a Writer mid-stream cancels
   workers and does not block forever.
7. `finish()` joins all workers cleanly and returns the
   underlying writer via the round-trip handoff channel
   (option E1 above). Verify via a unit test that
   `let w = writer.finish()?` returns the same `W` that was
   passed to `with_options`.

## Open questions

1. **`Drop` of Writer.** If the user forgets `finish()`, what
   happens? Options: (a) silently drop (lose data, bad), (b)
   spawn a detached thread to flush (unbounded delay, bad), (c)
   set a sentinel and let `panic_on_drop` warn (intrusive). Match
   Go: drop loses data. Document loudly. Add a debug-only
   `panic_on_drop` for tests.
2. **Thread pool size on `available_parallelism() == 1`.** Skip
   the worker pool entirely — fall through to stage C
   single-threaded path. Document.
3. **NUMA / pinning.** Not in scope. If a user shows a real
   perf issue, revisit with `affinity` crate.
4. **`tokio`-style async API.** Out of scope. MinLZ users are
   typically sync workloads; an async wrapper can be added in a
   separate crate without changing this one.
5. **Worker panic recovery.** Default: a worker panic poisons the
   Writer; subsequent calls return `Error::WorkerPanicked`.
   Verify with a test that intentionally panics inside a worker.
