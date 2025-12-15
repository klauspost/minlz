//! Test file definitions and data structures for benchmarking
//!
//! This module contains the test file configurations copied directly from
//! the Go implementation's benchmarks_test.go, ensuring identical test data.

/// Test file configuration matching Go's testFiles structure
#[derive(Debug, Clone)]
pub struct TestFile {
    pub label: &'static str,
    pub filename: &'static str,
    pub size_limit: usize,
}

/// Test files array copied directly from Go's benchmarks_test.go
/// These values are from https://raw.githubusercontent.com/google/snappy/master/snappy_unittest.cc
pub const TEST_FILES: &[TestFile] = &[
    TestFile { label: "html", filename: "html", size_limit: 0 },
    TestFile { label: "urls", filename: "urls.10K", size_limit: 0 },
    TestFile { label: "jpg", filename: "fireworks.jpeg", size_limit: 0 },
    TestFile { label: "jpg_200b", filename: "fireworks.jpeg", size_limit: 200 },
    TestFile { label: "pdf", filename: "paper-100k.pdf", size_limit: 0 },
    TestFile { label: "html4", filename: "html_x_4", size_limit: 0 },
    TestFile { label: "txt1", filename: "alice29.txt", size_limit: 0 },
    TestFile { label: "txt2", filename: "asyoulik.txt", size_limit: 0 },
    TestFile { label: "txt3", filename: "lcet10.txt", size_limit: 0 },
    TestFile { label: "txt4", filename: "plrabn12.txt", size_limit: 0 },
    TestFile { label: "pb", filename: "geo.protodata", size_limit: 0 },
    TestFile { label: "gaviota", filename: "kppkn.gtb", size_limit: 0 },
    TestFile { label: "txt1_128b", filename: "alice29.txt", size_limit: 128 },
    TestFile { label: "txt1_1000b", filename: "alice29.txt", size_limit: 1000 },
    TestFile { label: "txt1_10000b", filename: "alice29.txt", size_limit: 10000 },
    TestFile { label: "txt1_20000b", filename: "alice29.txt", size_limit: 20000 },
];

/// The canonical URL for benchmark data files (same as Go implementation)
pub const BENCH_URL: &str = "https://raw.githubusercontent.com/google/snappy/master/testdata/";

impl TestFile {
    /// Get the full URL for downloading this test file
    pub fn url(&self) -> String {
        format!("{}{}", BENCH_URL, self.filename)
    }

    /// Apply size limit to data if specified
    pub fn apply_size_limit(&self, data: Vec<u8>) -> Vec<u8> {
        if self.size_limit > 0 && data.len() > self.size_limit {
            data.into_iter().take(self.size_limit).collect()
        } else {
            data
        }
    }
}