//! Block-level integration tests, ported from `minlz_test.go`.

use super::*;
use crate::Error;

/// Helper: encode + decode at a given level and compare.
fn roundtrip_at(input: &[u8], level: Level) -> Result<(), String> {
    let mut enc = Vec::new();
    encode(&mut enc, input, level).map_err(|e| format!("encode: {e:?}"))?;
    let mut dec = Vec::new();
    decode(&mut dec, &enc).map_err(|e| format!("decode: {e:?}"))?;
    if dec != input {
        return Err(format!(
            "mismatch: got {} bytes, want {} bytes",
            dec.len(),
            input.len()
        ));
    }
    Ok(())
}

/// Run a round-trip at every shipping level.
fn roundtrip(input: &[u8]) -> Result<(), String> {
    // Only Fastest is implemented in this stage; Balanced/Smallest fall back
    // to uncompressed.  Both should still roundtrip.
    for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
        roundtrip_at(input, level).map_err(|e| format!("lvl {level:?}: {e}"))?;
    }
    Ok(())
}

/// Ports `TestMaxEncodedLen` from `minlz_test.go:45`.
#[test]
fn max_encoded_len_matches_go() {
    assert_eq!(max_encoded_len(0), Some(1));
    assert_eq!(max_encoded_len(1 << 5), Some((1 << 5) + 2));
    assert_eq!(max_encoded_len(MAX_BLOCK_SIZE), Some(MAX_BLOCK_SIZE + 2));
    assert_eq!(max_encoded_len(u32::MAX as usize), None);
    let sweep = if cfg!(miri) { 64 } else { 4096 };
    for i in 1..=sweep {
        assert_eq!(max_encoded_len(i), Some(i + 2));
    }
}

/// Ports `TestEmpty` from `minlz_test.go:199`.
#[test]
fn empty() {
    roundtrip(&[]).expect("empty roundtrip");
}

/// Ports `TestSmallCopy` from `minlz_test.go:205`.
#[test]
fn small_copy() {
    for i in 0..32 {
        let mut s = String::from("aaaa");
        s.push_str(&"b".repeat(i));
        s.push_str("aaaabbbb");
        roundtrip(s.as_bytes()).unwrap_or_else(|e| panic!("i={i}: {e}"));
    }
}

/// Cap iteration count under miri so tests still finish in reasonable time.
/// (Miri is 50-100× slower than native.)
fn cap(n: usize) -> usize {
    if cfg!(miri) {
        n.min(512)
    } else {
        n
    }
}

/// Ports `TestSmallRand` from `minlz_test.go:218`.
#[test]
fn small_rand() {
    let mut rng = Xor64::new(1);
    let mut n = 1;
    while n < cap(20_000) {
        let buf: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        roundtrip(&buf).unwrap_or_else(|e| panic!("n={n}: {e}"));
        n += 23;
    }
}

/// Ports `TestSmallRegular` from `minlz_test.go:231`.
#[test]
fn small_regular() {
    let mut n = 1;
    while n < cap(20_000) {
        let buf: Vec<u8> = (0..n).map(|i| ((i % 10) as u8) + b'a').collect();
        roundtrip(&buf).unwrap_or_else(|e| panic!("n={n}: {e}"));
        n += 23;
    }
}

/// Ports `TestSmallRepeat` from `minlz_test.go:243`.
#[test]
fn small_repeat() {
    let mut n = 1;
    while n < cap(20_000) {
        let mut buf = vec![0u8; n];
        let half = n / 2;
        for (i, b) in buf[..half].iter_mut().enumerate() {
            *b = ((i * 255) / n) as u8;
        }
        for (i, b) in buf[half..].iter_mut().enumerate() {
            *b = ((i % 10) as u8) + b'a';
        }
        roundtrip(&buf).unwrap_or_else(|e| panic!("n={n}: {e}"));
        n += 23;
    }
}

/// Ports `TestInvalidVarint` from `minlz_test.go:258`.
///
/// In the Rust port (no Snappy/S2 fallback), the first three inputs all have
/// non-zero first bytes and are rejected outright with `Corrupt`.  We add
/// equivalent inputs prefixed with `\x00` so the varint logic itself is also
/// exercised.
#[test]
fn invalid_varint() {
    let cases: &[&[u8]] = &[
        // Inputs from Go (no leading 0 — rejected as not-minlz).
        b"\xff",
        b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x00",
        b"\x80\x80\x80\x80\x10",
        // MinLZ-prefixed equivalents.
        b"\x00\xff",
        b"\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x00",
        b"\x00\x80\x80\x80\x80\x10",
    ];
    for input in cases {
        let err = decoded_len(input).expect_err("expected an error");
        assert_eq!(err, Error::Corrupt, "input={input:?}");
    }
}

/// Decoded zero-byte block accepts `[0x00]` and refuses `[]`.
#[test]
fn decoded_len_minimal() {
    assert_eq!(decoded_len(&[0x00]).unwrap(), 0);
    assert_eq!(decoded_len(&[]).unwrap_err(), Error::Corrupt);
    // `[0x00, 0x00]` declares length 0 with no body — Go rejects this.
    assert_eq!(decoded_len(&[0x00, 0x00]).unwrap_err(), Error::Corrupt);
    // `[0x00, 0x00, 'A']` decodes to a 1-byte literal block.
    assert_eq!(decoded_len(&[0x00, 0x00, b'A']).unwrap(), 1);
}

/// 0-byte and 1-byte inputs survive the round-trip at every level.
#[test]
fn tiny_roundtrips() {
    roundtrip(b"").unwrap();
    roundtrip(b"a").unwrap();
    roundtrip(b"ab").unwrap();
    roundtrip(b"abc").unwrap();
}

/// Mid-size deterministic round-trip exercising L1's 64K hash table.
#[test]
#[cfg_attr(miri, ignore = "32 KiB input × 3 levels is impractical under miri")]
fn medium_64k_roundtrip() {
    // 32 KiB of period-7 data — should compress heavily.
    let mut buf = Vec::with_capacity(32 << 10);
    while buf.len() < 32 << 10 {
        buf.extend_from_slice(b"foobar ");
    }
    roundtrip(&buf).unwrap();
}

/// Large round-trip exercising L1's >64K hash table.
#[test]
#[cfg_attr(miri, ignore = "128 KiB input × 3 levels is impractical under miri")]
fn large_big_roundtrip() {
    // 128 KiB of period-13 data.
    let mut buf = Vec::with_capacity(128 << 10);
    while buf.len() < 128 << 10 {
        buf.extend_from_slice(b"the quick brown fox ");
    }
    roundtrip(&buf).unwrap();
}

/// English pangram concatenated 100x — input that the Go noasm encoder
/// compresses heavily (≈99% reduction).  Used in two tests below.
const PANGRAM_PARTS: &[&[u8]] = &[
    b"The quick brown fox jumps over the lazy dog. ",
    b"Pack my box with five dozen liquor jugs. ",
    b"How quickly daft jumping zebras vex. ",
    b"Sphinx of black quartz, judge my vow. ",
    b"The five boxing wizards jump quickly.",
];

fn pangram_x(times: usize) -> Vec<u8> {
    let pat_len: usize = PANGRAM_PARTS.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(pat_len * times);
    for _ in 0..times {
        for p in PANGRAM_PARTS {
            out.extend_from_slice(p);
        }
    }
    out
}

/// Encoder must actually compress varied repetitive input (parity vs Go noasm).
///
/// Pure period-7 data (e.g. "foobar " * N) is *not* compressed by either
/// the Go `noasm` encoder or this Rust port: the single huge run-length
/// match exceeds `dstLimit`, both implementations return 0 → uncompressed
/// fallback.  The varied pangram input doesn't have this pathology.
#[test]
#[cfg_attr(miri, ignore = "20 KiB input is impractical under miri")]
fn encoder_compresses_natural_repetition() {
    let buf = pangram_x(100);
    let mut enc = Vec::new();
    encode(&mut enc, &buf, Level::Fastest).unwrap();
    assert!(
        enc.len() < buf.len() / 10,
        "expected heavy compression, got enc={} src={}",
        enc.len(),
        buf.len()
    );
}

/// Differential: decode blocks produced by the Go `noasm` encoder
/// (`encode_l1.go:encodeBlockGo` / `encodeBlockGo64K`) and verify the
/// plaintext matches.  Fixtures captured with `go run -tags=noasm`.
#[test]
#[cfg_attr(miri, ignore = "200 KiB pangram fixture is impractical under miri")]
fn decode_go_noasm_fixtures() {
    let pangram_100 = pangram_x(100);
    let pangram_1000 = pangram_x(1000);
    let fixtures: &[(&[u8], &str)] = &[
        (
            &pangram_100,
            "00d89a01e89a54686520717569636b2062726f776e20666f78206a756d7073206f76657220746865206c617a7920646f672e205061636b206d7920626f782077697468206669766520646f7a656e206c6971756f72206a7567732e20486f7720717569636b6c792064616674206a756d70696e67207a6562726173207665782e20537068696e78206f6620626c61636b2071756172747a2c206a75646765206d7920766f772e20546865206669766520626f78696e672077697a617264730529492ef386006c792ef4634c2869636b6c792e",
        ),
        (
            &pangram_1000,
            "00f08a0ce88354686520717569636b2062726f776e20666f78206a756d7073206f76657220746865206c617a7920646f672e205061636b206d7920626f782077697468206669766520646f7a656e206c6971756f72206a7567732e20486f7720717569636b6c792064616674206a756d70696e67207a6562726173207665782e20537068696e78206f6620626c61636b2071756172747a2c206a75646765206d7920766f772e200128b86669766520626f78696e672077697a61726473206a756d70d118e386002efc7b04032869636b6c792e",
        ),
    ];
    for &(plain, hex_enc) in fixtures {
        let enc = hex_decode(hex_enc);
        let mut dec = Vec::new();
        decode(&mut dec, &enc).expect("decode go fixture");
        assert_eq!(dec.len(), plain.len(), "decoded length mismatch");
        assert_eq!(&dec[..], plain, "decoded contents mismatch");
    }
}

/// Differential: decode blocks produced by the Go ASM encoder.  The ASM
/// path uses different heuristics than `encodeBlockGo`, so this exercises
/// the decoder against a *different* valid wire format than our own
/// encoder produces.
#[test]
#[cfg_attr(miri, ignore = "32 KiB foobar fixture is impractical under miri")]
fn decode_go_asm_fixtures() {
    let foobar = repeat_bytes(b"foobar ", 32 << 10);
    let abc = repeat_bytes(b"abc", 16 << 10);
    let alpha = repeat_bytes(b"abcdefghijklmnopqrstuvwxyz", 1024);
    let fixtures: &[(&[u8], &str)] = &[
        (&foobar, "0080800230666f6f62617220b901f4c97f"),
        (&abc, "0080800110616263b900f4cd3f"),
        (
            &alpha,
            "008008c86162636465666768696a6b6c6d6e6f707172737475767778797a7906f4b603",
        ),
    ];
    for &(plain, hex_enc) in fixtures {
        let enc = hex_decode(hex_enc);
        let mut dec = Vec::new();
        decode(&mut dec, &enc).expect("decode go fixture");
        assert_eq!(dec.len(), plain.len(), "decoded length mismatch");
        assert_eq!(&dec[..], plain, "decoded contents mismatch");
    }
}

fn repeat_bytes(pat: &[u8], n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n + pat.len());
    while v.len() < n {
        v.extend_from_slice(pat);
    }
    v.truncate(n);
    v
}

fn hex_decode(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Ports `TestDecodeBlockOverlapping` from `decode_test.go:91`.
/// Tests RLE-style patterns (overlapping forward copies).
#[test]
#[cfg_attr(
    miri,
    ignore = "1 KiB-pattern roundtrips × 5 × 3 levels too slow for miri"
)]
fn decode_block_overlapping() {
    let patterns: &[Vec<u8>] = &[
        repeat_bytes(b"a", 1000),
        repeat_bytes(b"ab", 1000),
        repeat_bytes(b"abc", 999),
        repeat_bytes(b"abcd", 1000),
        repeat_bytes(b"abcdefg", 1001),
    ];
    for (i, pat) in patterns.iter().enumerate() {
        roundtrip(pat).unwrap_or_else(|e| panic!("pattern {i}: {e}"));
    }
}

/// Ports `TestDecodeBlockLongOffsets` from `decode_test.go:123`.
/// Tests Copy2/Copy3 offsets.
#[test]
#[cfg_attr(miri, ignore = "200 KiB random input too slow for miri")]
fn decode_block_long_offsets() {
    let size = 200_000;
    let mut data = vec![0u8; size];
    let mut rng = Xor64::new(123);
    for b in data.iter_mut() {
        *b = rng.next() as u8;
    }
    let pattern = b"REPEATED_PATTERN_DATA";
    for &off in &[100usize, 1000, 10_000, 65_000, 100_000] {
        if off + pattern.len() < size {
            data[off..off + pattern.len()].copy_from_slice(pattern);
            if off * 2 + pattern.len() < size {
                data[off * 2..off * 2 + pattern.len()].copy_from_slice(pattern);
            }
        }
    }
    roundtrip(&data).unwrap();
}

/// Ports `TestEncodeNoiseThenRepeats` from `minlz_test.go:783`.
/// First half incompressible, second half compressible — total < 75 % of input.
#[test]
#[cfg_attr(miri, ignore = "256 KiB+ inputs too slow for miri")]
fn encode_noise_then_repeats() {
    for &orig_len in &[256 * 1024usize, 2048 * 1024] {
        let mut src = vec![0u8; orig_len];
        let mut rng = Xor64::new(1);
        let half = orig_len / 2;
        for b in src[..half].iter_mut() {
            *b = rng.next() as u8;
        }
        for (i, b) in src[half..].iter_mut().enumerate() {
            *b = (i >> 8) as u8;
        }
        let mut enc = Vec::new();
        encode(&mut enc, &src, Level::Fastest).unwrap();
        let want_less_than = orig_len * 3 / 4;
        assert!(
            enc.len() < want_less_than,
            "orig_len={orig_len}: got {} encoded bytes, want less than {want_less_than}",
            enc.len()
        );
    }
}

/// Ports `TestEncodeHuge` from `encode_test.go:26`.
/// Encode + decode at MAX_BLOCK_SIZE for every level.
#[test]
#[cfg_attr(miri, ignore = "8 MiB input × 3 levels is impractical under miri")]
fn encode_huge() {
    let data = vec![0u8; MAX_BLOCK_SIZE];
    for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
        let mut enc = Vec::new();
        encode(&mut enc, &data, level).unwrap();
        let mut dec = Vec::new();
        decode(&mut dec, &enc).unwrap();
        assert_eq!(dec.len(), data.len(), "level {level:?}");
        assert!(enc.len() <= max_encoded_len(data.len()).unwrap());
    }
}

/// Ports `TestSizes` from `encode_test.go:75`.
/// `emit_copy_size(offset, length)` must equal `emit_copy(...)`'s output length
/// for every length 4..MAX_BLOCK_SIZE at three representative offsets.  Same
/// for `emit_repeat_size`.
#[test]
#[cfg_attr(miri, ignore = "MAX_BLOCK_SIZE sweep is impractical under miri")]
fn emit_size_predictors() {
    // Loop ranges trimmed for speed; full sweep covered by TestSizes in CI.
    let offsets = [10usize, 4000, 70_000];
    let mut tmp = vec![0u8; 16];
    let lengths: Vec<usize> = (4..16).chain((16..MAX_BLOCK_SIZE).step_by(257)).collect();
    for &offset in &offsets {
        for &length in &lengths {
            // emit_copy actual size
            let n = super::emit::emit_copy(&mut tmp, offset, length);
            let predicted = super::emit::emit_copy_size(offset, length);
            assert_eq!(
                n, predicted,
                "emit_copy_size(offset={offset}, length={length}): got {predicted}, want {n}"
            );
        }
    }
    // emit_repeat
    for length in 1..16 {
        let n = super::emit::emit_repeat(&mut tmp, length);
        let predicted = super::emit::emit_repeat_size(length);
        assert_eq!(
            n, predicted,
            "emit_repeat_size({length}): got {predicted}, want {n}"
        );
    }
    for length in (16..MAX_BLOCK_SIZE).step_by(257) {
        let n = super::emit::emit_repeat(&mut tmp, length);
        let predicted = super::emit::emit_repeat_size(length);
        assert_eq!(
            n, predicted,
            "emit_repeat_size({length}): got {predicted}, want {n}"
        );
    }
}

/// Try to locate `testdata/<name>` in the upstream Go repo.  Honours the
/// `MINLZ_TESTDATA` env override so CI can point at a different path.
/// Returns `None` if not found (tests then skip with an `eprintln`).
fn locate_testdata(name: &str) -> Option<Vec<u8>> {
    let candidates: Vec<std::path::PathBuf> = {
        let mut v = Vec::new();
        if let Ok(d) = std::env::var("MINLZ_TESTDATA") {
            v.push(std::path::PathBuf::from(d).join(name));
        }
        // crates/minlz/  -> minlz-rs/  -> minlz/ (Go repo root)
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        v.push(manifest.join("../../../testdata").join(name));
        v.push(manifest.join("../../testdata").join(name));
        v
    };
    for p in &candidates {
        if let Ok(b) = std::fs::read(p) {
            return Some(b);
        }
    }
    None
}

/// Ports the block-decode portion of `TestDecodeGoldenInput`
/// (`minlz_test.go:635`).  Reads the checked-in `Mark.Twain-Tom.Sawyer.txt.mzb`
/// and decodes against the plaintext.
#[test]
#[cfg_attr(
    miri,
    ignore = "filesystem I/O + 14 KiB decode is impractical under miri"
)]
fn decode_golden_mzb_block() {
    let Some(plain) = locate_testdata("Mark.Twain-Tom.Sawyer.txt") else {
        eprintln!("skip: testdata Mark.Twain-Tom.Sawyer.txt not found");
        return;
    };
    let Some(mzb) = locate_testdata("Mark.Twain-Tom.Sawyer.txt.mzb") else {
        eprintln!("skip: testdata Mark.Twain-Tom.Sawyer.txt.mzb not found");
        return;
    };
    let mut dec = Vec::new();
    decode(&mut dec, &mzb).expect("decode mzb");
    assert_eq!(dec.len(), plain.len());
    assert_eq!(&dec[..], &plain[..]);
}

/// Block round-trip across all levels on a realistic English-text input
/// (the Twain corpus).  This is the block-only subset of Go's
/// `TestRoundtrips` (`minlz_test.go:1477`).
#[test]
#[cfg_attr(miri, ignore = "14 KiB Twain × 3 levels is impractical under miri")]
fn roundtrip_twain_corpus() {
    let Some(plain) = locate_testdata("Mark.Twain-Tom.Sawyer.txt") else {
        eprintln!("skip: testdata Mark.Twain-Tom.Sawyer.txt not found");
        return;
    };
    for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
        let mut enc = Vec::new();
        encode(&mut enc, &plain, level).unwrap();
        assert!(
            enc.len() < plain.len() * 4 / 5,
            "level {level:?}: ratio {} too poor",
            enc.len() as f64 / plain.len() as f64
        );
        let mut dec = Vec::new();
        decode(&mut dec, &enc).unwrap();
        assert_eq!(dec, plain, "level {level:?}");
    }
}

/// Ports the `copy1` sub-test of `TestEmitters` (`encode_test.go:108`).
/// Sweep every (offset, length) for Copy1 and parse the output back.
#[test]
#[cfg_attr(miri, ignore = "281K iterations too slow for miri")]
fn emitters_copy1() {
    let mut tmp = [0u8; 16];
    for off in 1..=1024usize {
        for l in 4..=273usize {
            let n = super::emit::emit_copy(&mut tmp, off, l);
            let bytes = &tmp[..n];
            // Tag in low 2 bits.
            assert_eq!(bytes[0] & 3, 0b01, "tag at off={off} len={l}");
            let mut length = ((bytes[0] >> 2) & 15) as usize;
            let offset = ((u16::from_le_bytes([bytes[0], bytes[1]]) >> 6) as usize) + 1;
            let consumed = if length == 15 {
                length = bytes[2] as usize + 18;
                3
            } else {
                length += 4;
                2
            };
            assert_eq!(length, l, "length parse at off={off} expected={l}");
            assert_eq!(offset, off, "offset parse at off={off} length={l}");
            assert_eq!(consumed, n, "byte count at off={off} length={l}");
        }
    }
}

/// Ports the `copy2-lits` sub-test of `TestEmitters` (`encode_test.go:195`).
/// 1-4 fused literals at every copy length 4..=11.
#[test]
#[cfg_attr(miri, ignore = "large offset sweep too slow for miri")]
fn emitters_copy2_lits() {
    let mut tmp = [0u8; 16];
    let mut lits: Vec<u8> = vec![1];
    while lits.len() <= 4 {
        for off in (super::format::MIN_COPY2_OFFSET..=super::format::MAX_COPY2_OFFSET).step_by(997)
        {
            for l in 4..=11usize {
                let n = super::emit::emit_copy_lits2(&mut tmp, &lits, off, l);
                let bytes = &tmp[..n];
                assert_eq!(bytes[0] & 3, 0b11, "tag");
                assert_eq!(bytes[0] & 4, 0, "copy3 bit set unexpectedly");
                let value = bytes[0] >> 3;
                let offset = (u16::from_le_bytes([bytes[1], bytes[2]]) as usize)
                    + super::format::MIN_COPY2_OFFSET;
                let lit_len = ((value & 3) as usize) + 1;
                let copy_len = ((value >> 2) as usize) + 4;
                assert_eq!(copy_len, l, "copy_len at off={off} l={l}");
                assert_eq!(offset, off, "offset at off={off} l={l}");
                assert_eq!(lit_len, lits.len());
                assert_eq!(n, 3 + lits.len());
                assert_eq!(&bytes[3..], &lits[..]);
            }
        }
        lits.push((lits.len() + 1) as u8);
    }
}

/// Ports the `copy3` sub-test of `TestEmitters` (`encode_test.go:358`).
/// Tests `emit_copy_lits3` / `emit_copy` for Copy3 range.
#[test]
#[cfg_attr(miri, ignore = "large length sweep too slow for miri")]
fn emitters_copy3() {
    let mut tmp = [0u8; 16];
    let mut lits: Vec<u8> = Vec::new();
    while lits.len() <= 3 {
        let mut off = super::format::MAX_COPY2_OFFSET + 1;
        while off <= super::format::MAX_COPY3_OFFSET {
            // Length sweep with geometric growth.
            let mut l = 4usize;
            while l <= 1 << 16 {
                let n = if !lits.is_empty() {
                    super::emit::emit_copy_lits3(&mut tmp, &lits, off, l)
                } else {
                    super::emit::emit_copy(&mut tmp, off, l)
                };
                let bytes = &tmp[..n];
                assert_eq!(
                    bytes[0] & 7,
                    super::format::TAG_COPY3,
                    "tag at off={off} l={l}"
                );
                let val = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let length_field = ((val >> 5) & 63) as usize;
                let offset = ((val >> 11) as usize) + super::format::MIN_COPY3_OFFSET;
                let (length, consumed) = match length_field {
                    0..=60 => (length_field + 4, 4),
                    61 => (bytes[4] as usize + 64, 5),
                    62 => (u16::from_le_bytes([bytes[4], bytes[5]]) as usize + 64, 6),
                    63 => (
                        ((bytes[4] as usize)
                            | ((bytes[5] as usize) << 8)
                            | ((bytes[6] as usize) << 16))
                            + 64,
                        7,
                    ),
                    _ => panic!("bad length field"),
                };
                let nlits = ((val >> 3) & 3) as usize;
                assert_eq!(nlits, lits.len(), "lit count at off={off} l={l}");
                if nlits > 0 {
                    assert_eq!(&bytes[consumed..consumed + nlits], &lits[..]);
                }
                assert_eq!(length, l, "length at off={off} l={l}");
                assert_eq!(offset, off, "offset at off={off} l={l}");
                assert_eq!(consumed + nlits, n, "total bytes at off={off} l={l}");
                l = ((l as f64) * 1.5) as usize + 1;
            }
            off = off.saturating_mul(2);
            if off == 0 {
                break;
            }
        }
        lits.push((lits.len() + 1) as u8);
    }
}

/// Smoke test: decode every file in the cargo-fuzz seed corpus directory
/// (if present).  Equivalent to running `decode_arbitrary` for one
/// iteration per seed — must not panic or read OOB on any input.
#[test]
#[cfg_attr(
    miri,
    ignore = "thousands of seeds — fuzz the focused miri_decoder test instead"
)]
fn decode_arbitrary_corpus_smoke() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("fuzz/corpus/decode_arbitrary");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skip: corpus not extracted at {}", dir.display());
        eprintln!("  run `bash crates/minlz/fuzz/seed.sh` to populate it");
        return;
    };
    let mut count = 0usize;
    let mut buf = Vec::with_capacity(1 << 20);
    for ent in entries.flatten() {
        let Ok(bytes) = std::fs::read(ent.path()) else {
            continue;
        };
        buf.clear();
        // Any error is acceptable; a panic is not.
        let _ = decode(&mut buf, &bytes);
        count += 1;
    }
    eprintln!("decode_arbitrary_corpus_smoke: scanned {count} seeds");
    assert!(
        count > 0,
        "corpus directory exists but had no readable files"
    );
}

/// Smoke test: round-trip every file in the cargo-fuzz roundtrip seed
/// corpus at every level.  Validates the encoder doesn't produce
/// uncompiable / un-decodable output for any seed.
#[test]
#[cfg_attr(
    miri,
    ignore = "thousands of seeds — fuzz the focused miri_decoder test instead"
)]
fn roundtrip_corpus_smoke() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("fuzz/corpus/roundtrip");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skip: corpus not extracted at {}", dir.display());
        return;
    };
    let mut enc = Vec::new();
    let mut dec = Vec::new();
    let mut count = 0usize;
    for ent in entries.flatten() {
        let Ok(bytes) = std::fs::read(ent.path()) else {
            continue;
        };
        if bytes.len() > MAX_BLOCK_SIZE {
            continue;
        }
        for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
            enc.clear();
            encode(&mut enc, &bytes, level).expect("encode");
            dec.clear();
            decode(&mut dec, &enc).expect("decode");
            assert_eq!(dec, bytes, "level {level:?} seed {:?}", ent.path());
        }
        count += 1;
    }
    eprintln!("roundtrip_corpus_smoke: scanned {count} seeds across 3 levels");
    assert!(
        count > 0,
        "corpus directory exists but had no readable files"
    );
}

/// Smoke test: encoder output must never exceed `max_encoded_len` for any
/// seed.  This is the unit-test equivalent of the `max_encoded_len` fuzz.
#[test]
#[cfg_attr(
    miri,
    ignore = "thousands of seeds — fuzz the focused miri_decoder test instead"
)]
fn max_encoded_len_corpus_smoke() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("fuzz/corpus/max_encoded_len");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skip: corpus not extracted at {}", dir.display());
        return;
    };
    let mut enc = Vec::new();
    let mut count = 0usize;
    for ent in entries.flatten() {
        let Ok(bytes) = std::fs::read(ent.path()) else {
            continue;
        };
        let Some(mel) = max_encoded_len(bytes.len()) else {
            continue;
        };
        for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
            enc.clear();
            encode(&mut enc, &bytes, level).expect("encode");
            assert!(
                enc.len() <= mel,
                "level {level:?}: enc.len()={} mel={mel} input_len={}",
                enc.len(),
                bytes.len()
            );
        }
        count += 1;
    }
    eprintln!("max_encoded_len_corpus_smoke: scanned {count} seeds");
}

/// Focused miri target: exercise the decoder's unsafe paths on a small,
/// curated set of inputs that hits each tag class (literal, repeat,
/// copy1, copy2, copy2-fused, copy3) at least once.  Designed to run
/// fast enough under miri to give meaningful UB-detection coverage in
/// a few minutes.  Runs natively too — just much faster.
#[test]
fn miri_decoder_unsafe_paths() {
    // Round-trip each small input at L1 (exercises every encoder unsafe
    // load/store and every decoder tag class via emit_copy / emit_literal
    // / emit_repeat).
    let inputs: &[&[u8]] = &[
        // Empty + tiny.
        b"",
        b"a",
        b"ab",
        b"abcdefghijklmnop",            // 16 bytes — boundary of the LZ4 overshoot.
        b"abcdefghijklmnopq",           // 17 bytes — one past the boundary.
        // Repeated short pattern (exercises copy1 + small offsets).
        b"abcabcabcabcabcabcabcabcabcabc",
        // Mixed: prefix + RLE (exercises forward_copy_ptr — overlap path).
        b"prefix aaaaaaaaaaaaaaaa suffix",
        // Long-ish English (exercises copy2 + emit_literal extension).
        b"the quick brown fox jumps over the lazy dog. the quick brown fox jumps over the lazy dog.",
    ];
    for &input in inputs {
        for &level in &[Level::Fastest, Level::Balanced, Level::Smallest] {
            let mut enc = Vec::new();
            encode(&mut enc, input, level).expect("encode");
            let mut dec = Vec::new();
            decode(&mut dec, &enc).expect("decode");
            assert_eq!(
                dec.as_slice(),
                input,
                "level {level:?} on {} bytes",
                input.len()
            );
        }
    }

    // Decode-only: known Go-asm fixtures that hit copy3 + extended-length
    // tag paths the round-trip alone can miss.
    let alpha = repeat_bytes(b"abcdefghijklmnopqrstuvwxyz", 1024);
    let enc = hex_decode("008008c86162636465666768696a6b6c6d6e6f707172737475767778797a7906f4b603");
    let mut dec = Vec::new();
    decode(&mut dec, &enc).expect("decode alpha fixture");
    assert_eq!(dec.len(), alpha.len());
    assert_eq!(&dec[..], &alpha[..]);
}

/// Tiny RNG — same as Go's `math/rand` would do for these test sizes is
/// not required; reproducibility is what we want.
struct Xor64(u64);
impl Xor64 {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x as u32
    }
}
