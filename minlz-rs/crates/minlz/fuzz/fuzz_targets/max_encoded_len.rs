//! Encoder output must never exceed `max_encoded_len(src.len())` at any
//! level.  Port of the tail-buffer check in Go's `FuzzEncodingBlocks`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use minlz::{encode, max_encoded_len, Level};

fuzz_target!(|data: &[u8]| {
    let Some(mel) = max_encoded_len(data.len()) else {
        return;
    };
    for level in [Level::Fastest, Level::Balanced, Level::Smallest] {
        let mut enc = Vec::new();
        encode(&mut enc, data, level).expect("encode");
        assert!(
            enc.len() <= mel,
            "level {level:?}: encoded {} > max_encoded_len({}) = {mel}",
            enc.len(),
            data.len()
        );
    }
});
