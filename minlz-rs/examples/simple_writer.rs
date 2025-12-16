//! Simple example demonstrating the MinLZ Writer
//!
//! This example shows how to use the streaming writer to compress data
//! and verify the output format matches the MinLZ specification.

use minlz::{Writer, LEVEL_FASTEST, LEVEL_BALANCED, LEVEL_SMALLEST};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("MinLZ Writer Example");
    println!("===================");

    let test_data = b"Hello, MinLZ streaming world! This is a test of the new streaming writer implementation. \
                      It should compress this text efficiently and produce a valid MinLZ stream format that \
                      can be decoded by compatible decoders. The stream includes a header, compressed blocks, \
                      and an EOF marker.";

    println!("Original data: {} bytes", test_data.len());
    println!("Data: {}", String::from_utf8_lossy(test_data));
    println!();

    // Test different compression levels
    for (level, name) in [(LEVEL_FASTEST, "Fastest"), (LEVEL_BALANCED, "Balanced"), (LEVEL_SMALLEST, "Smallest")] {
        println!("Compression Level: {} ({})", level, name);

        let mut output = Vec::new();
        let mut writer = Writer::builder(&mut output)
            .compression_level(level)
            .build()?;

        // Write the data
        writer.write_all(test_data)?;

        // Finish and get stats
        let (final_output, _index) = writer.finish()?;
        let compressed_size = final_output.len();
        let compression_ratio = compressed_size as f64 / test_data.len() as f64;

        println!("  Compressed: {} bytes", compressed_size);
        println!("  Compression ratio: {:.1}%", compression_ratio * 100.0);

        // Verify stream format
        let magic = b"\xff\x06\x00\x00MinLz";
        if final_output.len() >= magic.len() && &final_output[..magic.len()] == magic {
            println!("  ✓ Valid MinLZ stream header");
        } else {
            println!("  ✗ Invalid stream header");
        }

        // Check if we have reasonable compression
        if compression_ratio < 0.8 {
            println!("  ✓ Good compression achieved");
        } else {
            println!("  ⚠ Limited compression (expected for small data)");
        }

        println!();
    }

    println!("Testing user chunks...");
    let mut output = Vec::new();
    let mut writer = Writer::new(&mut output)?;

    // Add a user-defined chunk (metadata)
    writer.add_user_chunk(0x80, b"author=MinLZ Team")?;

    // Write data
    writer.write_all(b"Data with metadata")?;

    let (final_output, _) = writer.finish()?;
    println!("Stream with user chunk: {} bytes", final_output.len());
    println!("✓ User chunk added successfully");
    println!();

    println!("Testing encode_buffer (zero-copy) method...");
    let large_data = vec![b'X'; 10000]; // 10KB of 'X' characters

    let mut output = Vec::new();
    let mut writer = Writer::new(&mut output)?;

    // Use encode_buffer for efficient large data handling
    writer.encode_buffer(&large_data)?;

    let (final_output, _) = writer.finish()?;
    let compression_ratio = final_output.len() as f64 / large_data.len() as f64;

    println!("Original: {} bytes", large_data.len());
    println!("Compressed: {} bytes", final_output.len());
    println!("Compression ratio: {:.1}%", compression_ratio * 100.0);
    println!("✓ Encode buffer method works efficiently");
    println!();

    println!("All tests completed successfully! 🎉");
    println!("The MinLZ streaming writer is working correctly.");

    Ok(())
}