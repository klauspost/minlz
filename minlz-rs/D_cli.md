# Stage D — Command-line Tool (`mz`)

> Read `README.md` first. The project-wide rules apply.
> Stage **C** (single-threaded stream) must be DONE.
> Stage D builds a *testing-oriented* CLI. We are **not** porting
> every flag from the Go `mz` tool — only what is needed to drive
> the test suite and to round-trip files against the Go reference.

## Goal

A binary crate that lets a developer:

* Compress and decompress files round-trip.
* Round-trip against the Go `mz` binary (encode in one, decode in
  the other).
* Run light micro-benchmarks (compress-throughput, decompress-
  throughput) without needing the full criterion harness.
* Verify on stdin / stdout for piped use.

The CLI exists primarily as a **regression test harness** for
stages A–C and later stages. The polished feature set of the Go
`mz` (search, prefixes, follow, recompression, http inputs, etc.)
is **not** in scope — that can come later as a separate, optional
"mz-full" binary.

## Reference

* `cmd/mz/main.go` — entry point, flag parsing, subcommands.
* `cmd/mz/compress.go` — compression options.
* `cmd/mz/decompress.go` — decompression options.
* `cmd/mz/search.go` — out of scope here; revisit in stage G if a
  CLI surface for search is wanted.

## In scope

Subcommands:

* `mz c [options] <input>` (or `mz compress`) — compress one or
  more files.
* `mz d [options] <input>` (or `mz decompress`, `mz cat`) —
  decompress one or more files.
* `mz verify <input>` — read and validate, discard output.
* `mz bench <input>` — repeat compress and/or decompress N times
  on the input; print throughput. Used for parity checks against
  Go.

Flags (subset of Go's):

| Rust flag             | Go equivalent                      |
|-----------------------|------------------------------------|
| `-1`, `-2`, `-3`      | level shortcut                     |
| `--level N`           | explicit level                     |
| `-0`                  | uncompressed mode                  |
| `--block-size N`      | `-bs`                              |
| `--pad N`             | `-pad`                             |
| `--block`             | `--block` (single-block mode, .mzb)|
| `-c, --stdout`        | `-c`                               |
| `-o FILE`             | `-o`                               |
| `--rm`                | `-rm`                              |
| `-q, --quiet`         | `-q`                               |
| `--verify`            | `--verify`                         |
| `--bench N`           | `--bench N` (decoder side)         |
| `--threads N`         | `-cpu` (only meaningful after E)   |
| `-`                   | use stdin/stdout for I/O           |

Flags **explicitly excluded** in stage D:

* `--xfast` (no SuperFast)
* `--recomp` (no fallback)
* HTTP input (`-` only for stdin; no `http://`)
* Wildcards / `filepathx.Glob` — let the shell do it. On Windows
  the user can use a small `for %f in (*.txt) do mz c %f` if
  needed.
* `--search*` (stage G feature, gate behind a `search` subcommand
  that does not exist yet)
* `--follow`
* CPU/mem/trace profiling flags (`-cpuprof`, `-memprof`,
  `-traceprof`) — use `cargo flamegraph` instead, or add later
  with `pprof-rs` if a real need emerges.

## CLI parser choice

Two reasonable choices:

1. **`clap` v4** — the de-facto Rust CLI library. ~30 KB binary
   size impact. Many transitive deps.
2. **Hand-rolled** — `std::env::args()` plus a small flag parser.
   The required surface is small (≤ 15 flags) and the Go style
   is straightforward.

**Decision: hand-rolled.** Following project rule 2 (minimal deps).
The CLI is small enough that a 150-line argument parser in
`bin/mz/src/args.rs` is cleaner than a clap dependency tree. If
future stages need richer help/auto-completion, revisit.

If hand-rolling becomes a drag, fallback choice: `lexopt` (single
file, zero deps, ≈ 200 lines). Note: this is one of the few cases
where adding a single tiny dep is reasonable.

## Layout

```
minlz-rs/bin/mz/
├── Cargo.toml          # bin crate, depends on minlz library
└── src/
    ├── main.rs         # subcommand dispatch
    ├── args.rs         # hand-rolled flag parser
    ├── compress.rs
    ├── decompress.rs
    ├── verify.rs
    └── bench.rs
```

Single binary, multiple modules. No workspace shenanigans.

## Behaviors that must match Go exactly

These are tested by piping output through the Go `mz` and vice
versa, so any deviation breaks the cross-impl test.

* Output filename extension: `.mz` (stream) or `.mzb` (single
  block). Match `cmd/mz/main.go:minlzExt`/`minlzBlockExt`.
* Stdin/stdout via `-` argument — same as Go.
* Block mode (`--block`) reads the entire input into memory (max
  8 MiB) and writes a single MinLZ block file. No stream framing,
  no CRC, no EOF.
* Exit codes: 0 on success, 2 on error (matches Go's `exitErr`).
* `--bench N` runs decompression N times and reports
  bytes/second; matches Go's `-bench` semantics in
  `decompress.go`.
* `--verify` reads and validates input, writes no output. Used
  to drive fuzz triage.

## I/O

* `std::io::BufReader` and `BufWriter` for file I/O. Buffer sizes
  ≥ 256 KiB so we are not the bottleneck.
* For stdin/stdout, wrap `io::stdin().lock()` / `io::stdout().lock()`
  in buffered adapters.
* Each `Reader`/`Writer` from the library is wrapped in the
  buffered I/O, not the other way around.

Watch out for: on Windows, stdin/stdout in binary mode requires
`set_binary` workaround. Use `os::windows::io::FromRawHandle` or
just document "use `--` redirection from PowerShell with
`-Encoding Byte`". Test on Windows in CI.

## Testing plan

### Integration tests in `bin/mz/tests/`

* `roundtrip.rs`:
  * For each level, compress a sample file via `mz c`, decompress
    via `mz d`, compare bytes. Uses `assert_cmd` (dev-dep) for
    process spawning — small, single-purpose, fine to add.
* `cross_impl.rs`:
  * If `MZ_GO_BIN` env var is set, compress with Rust `mz`,
    decompress with Go `mz`, and vice versa. Skipped in CI by
    default; gated test on the dev box.
* `stdin_stdout.rs`:
  * Pipe a file in via stdin, get compressed bytes out via stdout,
    pipe through `mz d -` and compare to original.

### Snapshot tests for help output

`mz`, `mz c --help`, `mz d --help` produce stable help text.
Snapshot via `insta` would be tempting but insta is a dep — match
literal strings instead, kept in a `.txt` fixture file.

### Manual tests (documented in stage doc)

Run before merging:

```
mz c testdata/alice29.txt -o alice.mz
mz d alice.mz -o alice.txt
diff testdata/alice29.txt alice.txt    # empty
mz verify alice.mz                      # exit 0
mz bench --bench 10 alice.mz            # prints MB/s
```

## Mapping table (Go subcommand → Rust)

| Go path / function              | Rust path / function           |
|---------------------------------|--------------------------------|
| `cmd/mz/main.go:main`           | `bin/mz/src/main.rs:main`      |
| `cmd/mz/compress.go:mainCompress`| `bin/mz/src/compress.rs:run`  |
| `cmd/mz/decompress.go:mainDecompress`| `bin/mz/src/decompress.rs:run`|
| `cmd/mz/main.go:openFile`       | `bin/mz/src/io.rs:open_input`  |
| `cmd/mz/main.go:toSize`         | `bin/mz/src/args.rs:parse_size`|

## Success criteria (DONE = all of these)

1. `cargo build --release -p mz` produces a working binary.
2. Round-trip tests pass: `mz c | mz d` returns input.
3. Cross-impl tests pass when `MZ_GO_BIN` is set (dev box).
4. Block mode (`--block`) produces files Go `mz d` can decompress
   and vice versa.
5. Stdin/stdout tests pass on Linux, macOS, and Windows in CI.
6. `mz --help` is readable and lists all supported flags.

## Open questions

1. **Should we ship `mz` to crates.io / cargo install?** Probably
   not until stage G is done. Decide at stage G review.
2. **`assert_cmd` as dev-dep.** Yes — it is the standard for
   testing CLI binaries; small surface, well-maintained.
3. **What does `mz bench` print exactly?** Match Go's
   `Compression: x->y bytes (z%), elapsed t (mb/s)` format so
   side-by-side comparison is grep-able.
4. **Multi-file input.** Stage D supports one file per invocation
   (plus `-` for stdin) to keep argv handling simple. Multi-file
   via shell loop. Revisit only if a real need emerges.
5. **Compressed-CRC variant (`chunkType == 0x03`).** Go's `mz`
   writer doesn't switch on this — it always emits 0x02. Match
   that. Stage F (index) and stage G (search) don't change this.
