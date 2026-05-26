# minlz fuzz harness

Three [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets, ports
of the block-level fuzzers from the Go reference repo.

| Target              | Mirrors                          | What it checks                                                |
|---------------------|----------------------------------|---------------------------------------------------------------|
| `decode_arbitrary`  | Go `FuzzDecodeBlock`             | Decoder never panics or reads OOB on any byte slice.          |
| `roundtrip`         | Go `FuzzEncodingBlocks` (rt)     | Encode + decode at every level reproduces the input.          |
| `max_encoded_len`   | Go `FuzzEncodingBlocks` (tail)   | Encoder output ≤ `max_encoded_len(src.len())` at every level. |

## Running

```
cargo install cargo-fuzz       # one-time, needs nightly toolchain
cd crates/minlz/fuzz
cargo +nightly fuzz run decode_arbitrary  -- -max_total_time=1200
cargo +nightly fuzz run roundtrip         -- -max_total_time=1200
cargo +nightly fuzz run max_encoded_len   -- -max_total_time=1200
```

## Seed corpora

The Go repo ships seed corpora under `testdata/fuzz/`:

| Rust target         | Go corpus                                                  |
|---------------------|------------------------------------------------------------|
| `decode_arbitrary`  | `testdata/fuzz/block-corpus-raw.zip`                       |
| `roundtrip`         | `testdata/fuzz/block-corpus-enc.zip`, `enc_regressions.zip` |
| `max_encoded_len`   | same as `roundtrip`                                        |

Unzip each into `corpus/<target>/` before running for full coverage.
