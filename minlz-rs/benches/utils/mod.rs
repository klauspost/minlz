//! Utilities for MinLZ benchmarks
//!
//! This module provides helper functions for downloading test data, reading files,
//! and data manipulation that mirrors the Go benchmark implementation.

use reqwest::blocking as reqwest;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub mod test_files;
pub use test_files::{TestFile, TEST_FILES};

/// Download a benchmark file if it doesn't exist locally
/// Returns the path to the local file
pub fn ensure_benchmark_file(
    test_file: &TestFile,
    bench_data_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Create benchmark data directory if it doesn't exist
    fs::create_dir_all(bench_data_dir)?;

    let local_path = bench_data_dir.join(test_file.filename);

    // Check if file already exists and has content
    if let Ok(metadata) = fs::metadata(&local_path) {
        if metadata.len() > 0 {
            return Ok(local_path);
        }
    }

    // Download the file
    println!(
        "Downloading test data: {} from {}",
        test_file.filename,
        test_file.url()
    );

    let response = reqwest::get(&test_file.url())?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to download {}: HTTP {}",
            test_file.url(),
            response.status()
        )
        .into());
    }

    let content = response.bytes()?;

    // Write to local file
    let mut file = fs::File::create(&local_path)?;
    file.write_all(&content)?;
    file.flush()?;

    println!(
        "Downloaded {} ({} bytes)",
        test_file.filename,
        content.len()
    );

    Ok(local_path)
}

/// Read a file and apply size limit if specified
pub fn read_test_file(
    test_file: &TestFile,
    bench_data_dir: &Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file_path = ensure_benchmark_file(test_file, bench_data_dir)?;
    let data = fs::read(&file_path)?;

    if data.is_empty() {
        return Err(format!("{} has zero length", file_path.display()).into());
    }

    Ok(test_file.apply_size_limit(data))
}

/// Read the Mark Twain test file for size-series benchmarks
pub fn read_twain_file() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Look for the file in standard locations
    let possible_paths = [
        "testdata/Mark.Twain-Tom.Sawyer.txt",
        "../testdata/Mark.Twain-Tom.Sawyer.txt",
        "../../testdata/Mark.Twain-Tom.Sawyer.txt",
        // Go project location
        "../../../testdata/Mark.Twain-Tom.Sawyer.txt",
        "../../../../testdata/Mark.Twain-Tom.Sawyer.txt",
    ];

    for path in &possible_paths {
        if let Ok(data) = fs::read(path) {
            if !data.is_empty() {
                return Ok(data);
            }
        }
    }

    Err("Mark.Twain-Tom.Sawyer.txt not found in any expected location".into())
}

/// Expand a source buffer to a given size, mutating copies (mirrors Go's expand function)
/// This creates test data of a specific size while introducing variations
pub fn expand(src: &[u8], n: usize) -> Vec<u8> {
    let mut dst = vec![0u8; n];
    let mut cnt = 0u8;
    let mut offset = 0;

    while offset < n {
        // Copy source data
        let copy_len = (n - offset).min(src.len());
        dst[offset..offset + copy_len].copy_from_slice(&src[..copy_len]);

        // Mutate the copied data with XOR
        for i in 0..copy_len {
            dst[offset + i] ^= cnt;
        }

        offset += copy_len;
        cnt = cnt.wrapping_add(1);
    }

    dst
}

/// Generate random data for random data benchmarks
pub fn generate_random_data(size: usize, seed: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let mut state = seed;

    for _ in 0..size {
        // Simple LCG for reproducible random data
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        data.push((state >> 8) as u8);
    }

    data
}

/// Get the benchmark data directory path
pub fn get_bench_data_dir() -> PathBuf {
    // Try to find a good location for benchmark data
    let paths = [
        "testdata/bench",
        "target/testdata/bench",
        "../testdata/bench",
        "../../testdata/bench",
    ];

    for path in &paths {
        let path_buf = PathBuf::from(path);
        if let Ok(_) = fs::create_dir_all(&path_buf) {
            return path_buf;
        }
    }

    // Fallback to current directory
    PathBuf::from("bench_data")
}
