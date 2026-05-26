//! Decode arbitrary bytes — the decoder must never panic or read OOB.
//!
//! Seed corpus from `testdata/fuzz/block-corpus-raw.zip` in the Go repo.
//!
//! Run with `cargo +nightly fuzz run decode_arbitrary -- -max_total_time=60`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut out = Vec::new();
    // We expect any of: Ok, Corrupt, TooLarge.  We do NOT expect a panic.
    let _ = minlz::decode(&mut out, data);
});
