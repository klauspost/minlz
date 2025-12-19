//! Single block decompression benchmark to compare with Go noasm performance

use crate::{encode, decode, LEVEL_FASTEST, LEVEL_BALANCED, LEVEL_SMALLEST};
use std::fs;
use std::time::Instant;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_single_block_decompression() -> Result<(), Box<dyn std::error::Error>> {
        // Read the kppkn.gtb test file
        let original_data = fs::read("testdata/bench/kppkn.gtb")?;
        println!("Original kppkn.gtb size: {} bytes ({:.1} KB)",
                original_data.len(),
                original_data.len() as f64 / 1024.0);

        // Test if it's a block API issue by trying streaming API
        println!("\n=== TESTING STREAMING API ===");
        use crate::Writer;
        use std::io::{Cursor, Write};

        let mut stream_compressed = Vec::new();
        {
            let cursor = Cursor::new(&mut stream_compressed);
            let mut writer = Writer::builder(cursor)
                .compression_level(LEVEL_FASTEST)
                .build()?;
            writer.write_all(&original_data)?;
            writer.finish()?;
        }
        println!("Streaming encoded {} bytes to {} bytes", original_data.len(), stream_compressed.len());

        // Test streaming decompression
        use crate::Reader;
        use std::io::Read;

        let cursor = Cursor::new(&stream_compressed);
        let mut reader = Reader::new(cursor)?;
        let mut stream_decompressed = Vec::new();
        reader.read_to_end(&mut stream_decompressed)?;

        if stream_decompressed == original_data {
            println!("Streaming round-trip: ✅ OK");
        } else {
            println!("Streaming round-trip: ❌ FAILED");
            return Err("Streaming round-trip failed".into());
        }

        // Now test block API
        println!("\n=== TESTING BLOCK API ===");
        let mut test_compressed = Vec::new();
        let mut test_decompressed = vec![0u8; original_data.len()];

        encode(&mut test_compressed, &original_data, LEVEL_FASTEST)?;
        println!("Block encoded {} bytes to {} bytes", original_data.len(), test_compressed.len());

        match decode(&mut test_decompressed, &test_compressed) {
            Ok(()) => {
                if test_decompressed == original_data {
                    println!("Block round-trip: ✅ OK");
                } else {
                    println!("Block round-trip: ❌ Data mismatch");
                    return Err("Block round-trip data mismatch".into());
                }
            },
            Err(e) => {
                println!("Block round-trip: ❌ Decode error: {:?}", e);
                return Err(e.into());
            }
        }

        // Compress with all three levels
        let mut compressed_l1 = Vec::new();
        let mut compressed_l2 = Vec::new();
        let mut compressed_l3 = Vec::new();

        encode(&mut compressed_l1, &original_data, LEVEL_FASTEST)?;
        encode(&mut compressed_l2, &original_data, LEVEL_BALANCED)?;
        encode(&mut compressed_l3, &original_data, LEVEL_SMALLEST)?;

        println!("Level 1 compressed: {} bytes ({:.2}%)",
                compressed_l1.len(),
                compressed_l1.len() as f64 / original_data.len() as f64 * 100.0);
        println!("Level 2 compressed: {} bytes ({:.2}%)",
                compressed_l2.len(),
                compressed_l2.len() as f64 / original_data.len() as f64 * 100.0);
        println!("Level 3 compressed: {} bytes ({:.2}%)",
                compressed_l3.len(),
                compressed_l3.len() as f64 / original_data.len() as f64 * 100.0);

        // Benchmark decompression (Level 1)
        let iterations = 1000;
        let mut decompressed = vec![0u8; original_data.len()];

        let start = Instant::now();
        for _ in 0..iterations {
            decode(&mut decompressed, &compressed_l1)?;
        }
        let duration = start.elapsed();

        let avg_time_ns = duration.as_nanos() / iterations as u128;
        let throughput_mb_s = (original_data.len() as f64 / 1_000_000.0) / (avg_time_ns as f64 / 1_000_000_000.0);

        println!("\n=== RUST SINGLE BLOCK DECOMPRESSION BENCHMARK ===");
        println!("Level 1 decompression:");
        println!("  Average time: {} ns/op", avg_time_ns);
        println!("  Throughput: {:.2} MB/s", throughput_mb_s);
        println!("  Compression ratio: {:.2}%", compressed_l1.len() as f64 / original_data.len() as f64 * 100.0);

        // Benchmark decompression (Level 2)
        let start = Instant::now();
        for _ in 0..iterations {
            decode(&mut decompressed, &compressed_l2)?;
        }
        let duration = start.elapsed();

        let avg_time_ns = duration.as_nanos() / iterations as u128;
        let throughput_mb_s = (original_data.len() as f64 / 1_000_000.0) / (avg_time_ns as f64 / 1_000_000_000.0);

        println!("Level 2 decompression:");
        println!("  Average time: {} ns/op", avg_time_ns);
        println!("  Throughput: {:.2} MB/s", throughput_mb_s);
        println!("  Compression ratio: {:.2}%", compressed_l2.len() as f64 / original_data.len() as f64 * 100.0);

        // Benchmark decompression (Level 3)
        let start = Instant::now();
        for _ in 0..iterations {
            decode(&mut decompressed, &compressed_l3)?;
        }
        let duration = start.elapsed();

        let avg_time_ns = duration.as_nanos() / iterations as u128;
        let throughput_mb_s = (original_data.len() as f64 / 1_000_000.0) / (avg_time_ns as f64 / 1_000_000_000.0);

        println!("Level 3 decompression:");
        println!("  Average time: {} ns/op", avg_time_ns);
        println!("  Throughput: {:.2} MB/s", throughput_mb_s);
        println!("  Compression ratio: {:.2}%", compressed_l3.len() as f64 / original_data.len() as f64 * 100.0);

        println!("\n=== COMPARISON TO GO NOASM ===");
        println!("Go noasm Level 1: 127388 ns/op, 1446.91 MB/s, 33.68%");
        println!("Go noasm Level 2: 164695 ns/op, 1119.16 MB/s, 28.62%");
        println!("Go noasm Level 3: 115236 ns/op, 1599.50 MB/s, 24.96%");

        // Verify correctness
        assert_eq!(&decompressed, &original_data);

        Ok(())
    }
}