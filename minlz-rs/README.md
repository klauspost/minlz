# MinLZ Rust Port — Implementation Plans

This folder contains the staged implementation plan for porting MinLZ from Go
to Rust. Each stage (A–G) is described in its own document and must be
**completed and merged before the next stage begins**.

The finished crate will live at **`github.com/minio/minlz-rs`** (separate
repo from the Go module at `github.com/minio/minlz`). The `minlz-rs/`
folder in the Go repo is a temporary planning scratchpad — once stage A
starts, the actual implementation moves to the dedicated Rust repo. Keep
that in mind when the plans reference paths: any `crates/minlz/...` path
in a stage doc is **relative to the eventual `minlz-rs` repo root**, not
to this scratchpad folder.

## Project-wide rules

These rules apply to **every** stage. Each per-stage document repeats the
rules that constrain its work, but this is the authoritative copy.

1. **Idiomatic Rust.** The Go layout is a reference, not a contract. Restructure
   into Rust modules, traits, error enums, builder patterns, iterators, etc.
   when the Rust idiom is clearer.
2. **Minimal external dependencies.** Pure-std is the default. A new crate may
   be added only after explicit discussion in its stage doc. Dev-deps for
   testing/benchmarking/fuzzing (criterion, cargo-fuzz, etc.) are allowed.
3. **Test coverage ≥ Go.** Every behavior covered by a Go test must be covered
   by a Rust test (translated, not auto-generated). Add Rust-specific tests
   wherever a port introduces new failure modes (lifetimes, overflow, unsafe).
4. **Performance target.** Within 5 % of the equivalent `noasm` Go path —
   not within 5 % of the assembly path. Measure with the same input corpus the
   Go benchmarks use. For decoder asm (stage B) the target shifts to the
   assembly variant.
5. **No out-of-scope features.** The following are **explicitly excluded** from
   the port:
   * LZ4 conversion (`lz4convert.go`, `internal/lz4ref/`)
   * `LevelSuperFast` (`encode_l0.go`, `encodeBlockFast*`)
   * Assembly encoders (`encodeBlockAsm*`, `encodeFastBlockAsm*`,
     `encodeBetterBlockAsm*`, `emitLiteral`/`emitCopy`/`matchLen`
     asm versions)
   * Snappy / S2 fallback (`Reader.allowFallback`, `s2.Decode` paths,
     `magicBodyS2`, `magicBodySnappy`, `chunkTypeLegacyCompressedData`)
   * **All dictionary encoding** (`dict.go`, every `*dict` parameter
     in `encode_l3.go`, the `maxDictSrcOffset` constant). No dict
     type, no dict API, no TODO breadcrumbs. The Rust public API
     must not even expose a `_ = dict` placeholder.
6. **Stage gating.** Stage N+1 may not start until N is *DONE*: optimized,
   tested, benchmarked, and documented. No partial stages, no
   "I'll come back to it".
7. **Reference by file + symbol, not by commit.** The branch the agent is
   placed on may change. When the spec or a Go function is referenced,
   always cite as `<file>:<symbol>` (e.g. `decode.go:minLZDecodeGo`) so the
   reference stays valid across branch changes. SPEC.md and SPEC_SEARCH.md
   are the canonical format definitions and should be re-read at the start
   of each stage.

## Scope summary

| Stage | Subject                                | Reference Go files                                                       |
|-------|----------------------------------------|--------------------------------------------------------------------------|
| A     | Block encoders L1–L3, safe decoder     | `minlz.go`, `encode.go`, `encode_l1.go`, `encode_l2.go`, `encode_l3.go`, `decode.go`, `internal/reference/` |
| B     | Assembly decoders (amd64, aarch64)     | `decode.go:minLZDecodeGo`, `asm_amd64.s` (decoder section), `asm_arm64.s`|
| C     | Single-threaded stream reader/writer   | `reader.go`, `writer.go` (concurrency == 1 paths), `SPEC.md`             |
| D     | CLI tool for testing                   | `cmd/mz/main.go`, `cmd/mz/compress.go`, `cmd/mz/decompress.go`           |
| E     | Multi-threaded stream reader/writer    | `reader.go:DecodeConcurrent`, `writer.go` (concurrency > 1 paths)        |
| F     | Stream index (write, read, seek)       | `index.go`, SPEC.md §4.12                                                |
| G     | Block search (build, read, query)      | `search_table.go`, `search_reader.go`, `search_index.go`, `search_compressed.go`, `SPEC_SEARCH.md`, `SEARCH.md` |

## Target crate layout

A single workspace under `minlz-rs/`. Stages add modules, not crates,
unless a stage doc explicitly proposes otherwise.

```
minlz-rs/
├── Cargo.toml              # workspace
├── crates/
│   └── minlz/              # library crate
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs
│       │   ├── error.rs
│       │   ├── block/      # stage A
│       │   ├── arch/       # stage B (cfg-gated)
│       │   ├── stream/     # stages C, E
│       │   ├── index.rs    # stage F
│       │   └── search/     # stage G
│       ├── examples/       # ad-hoc benches / smoke tools
│       ├── fuzz/           # cargo-fuzz crate (stand-alone manifest)
│       └── tests/          # integration tests + fixtures
└── bin/
    └── mz/                 # stage D
```

This layout is a **suggestion**, not binding. Each stage doc may revise
it. The key invariant is one library crate with cfg-gated modules.

### Stage-A layout actually delivered

```
minlz-rs/crates/minlz/
├── src/
│   ├── lib.rs, error.rs
│   └── block/{mod,format,load_store,hash,emit,match_len,decode,encode_l1,encode_l2,encode_l3,tests}.rs
├── examples/bench.rs                  # hand-rolled bench; criterion harness TBD
└── fuzz/                              # cargo-fuzz crate
    ├── Cargo.toml                     # stand-alone — has its own [workspace]
    ├── fuzz_targets/{decode_arbitrary,roundtrip,max_encoded_len}.rs
    ├── corpus/<target>/               # seeded via seed.sh
    ├── seed.sh                        # imports zip + Go fuzz-cache seeds
    ├── run.sh                         # one-shot driver, mirrors /mnt → ext4 on WSL
    ├── RUNBOOK.md                     # how to fuzz, triage, get coverage
    └── _seed_helper/                  # Go program that parses go-test-fuzz-v1 cache files
```

Stage B will fold `decode.rs` into `block/decode/{mod,safe,amd64,aarch64}.rs`.

## MSRV and toolchain

* Target stable Rust 1.75 or newer (pinned in `rust-toolchain.toml`).
  Specific MSRV bump must be justified in the stage that needs it.
* `#![forbid(unsafe_op_in_unsafe_fn)]` and `#![deny(unsafe_code)]` outside
  the explicit unsafe modules (block codec hot path may opt-in).
* `cargo clippy -- -D warnings` and `cargo fmt --check` must pass for each
  stage's PR.

## CI

Workflows live in `.github/workflows/`:

| File         | What it does                                                                                      |
|--------------|---------------------------------------------------------------------------------------------------|
| `ci.yml`     | `fmt --check`, `clippy -D warnings`, `cargo build` + `cargo test` (debug + release) on Linux / macOS / Windows × stable + MSRV (1.75).  `cargo doc` with `-D warnings`. |
| `miri.yml`   | `cargo +nightly miri test --lib` on push-to-main, weekly cron, and manual dispatch.  Heavy tests are `#[cfg_attr(miri, ignore)]`'d so the suite finishes in ~15 min. |
| `fuzz.yml`   | Short fuzz pass (5 min per target by default) on push-to-main and nightly cron.  Matrix over `decode_arbitrary` / `roundtrip` / `max_encoded_len`.  Corpus growth is cached across runs; crash artifacts upload to the workflow run on failure. |

Long-form fuzzing (≥ 20 min × 3 targets) is driven locally via
`crates/minlz/fuzz/run.sh` — see `crates/minlz/fuzz/RUNBOOK.md`.

## Testing & verification policy

* Every public function gets a unit test. Property tests via `proptest`
  (dev-dep) are encouraged when the input domain is large.
* Each stage **must** include a differential test against the Go reference
  implementation. Pre-generated Go output should be checked into
  `crates/minlz/tests/fixtures/` so the Rust CI does not need a Go
  toolchain.
* `cargo fuzz` corpora from `testdata/fuzz/` and `testdata/enc_regressions.zip`
  are reused. The fuzz targets live under `crates/minlz/fuzz/`.
* Benchmarks via `criterion` (dev-dep), reproducing the workloads in
  `benchmarks_test.go` (`testFiles` corpus). The Rust benchmark must
  print numbers comparable to the Go `noasm` benchmark (see
  `parallel-16-noasm.txt`).
* Decoder fuzz must include the "panic on any input" target: a random
  byte slice must never panic or read past the source / destination
  bounds. This already exists in Go (`FuzzDecode*`) and must be ported.

## Memory safety

* Hot paths may use `unsafe { ptr::read_unaligned }` and
  `unsafe { copy_nonoverlapping }` where the Go version uses
  `unsafe_enabled.go` helpers. Each `unsafe` block requires a
  `// SAFETY:` comment justifying the invariant.
* Public API must be safe. No `unsafe fn` exposed.
* Decoder must never read past `src` or write past `dst`. The Go
  decoder enforces this in the tail loop (`minLZDecodeGo` after
  `s < len(src)-11` exits); the Rust port must match.

## Open questions tracked across stages

Each stage doc has its own "Open questions" section. Cross-stage
questions are tracked here:

* **Crate name on crates.io.** The repo is `minio/minlz-rs`, but the
  crate name need not match the repo. Likely candidates: `minlz` (if
  available) or `minio-minlz`. Confirm before the first stage-A
  publish.
* **`no_std` support.** Stage A shipped `std`-only (uses `Vec` and
  `std::error::Error`). Conversion to `no_std + alloc` is mechanical
  (~20 lines of cfg).  Defer until a `no_std` consumer asks.
_(Dictionary support is fully out of scope per rule 5 above —
not an open question.)_
