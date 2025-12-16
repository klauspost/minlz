//! Parallel compression support for MinLZ
//!
//! This module implements multi-threaded block compression using background worker threads
//! and channels for communication. It maintains output order while allowing parallel processing.

use crate::{encode, Error, Result};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::collections::BTreeMap;

/// A work item representing a block to be compressed
#[derive(Debug)]
struct CompressionJob {
    /// Sequence number for maintaining output order
    sequence: u64,
    /// Input data to compress
    data: Vec<u8>,
    /// Compression level to use
    level: i32,
}

/// Result of a compression job
#[derive(Debug)]
struct CompressionResult {
    /// Sequence number matching the original job
    sequence: u64,
    /// Compressed data
    compressed: Vec<u8>,
    /// Original uncompressed data
    original_data: Vec<u8>,
    /// Error if compression failed
    error: Option<Error>,
}

/// Multi-threaded compression pool
pub struct CompressionPool {
    /// Channel for sending jobs to workers
    job_sender: Option<Sender<CompressionJob>>,
    /// Channel for receiving results from workers
    result_receiver: Receiver<CompressionResult>,
    /// Worker thread handles
    workers: Vec<thread::JoinHandle<()>>,
    /// Next sequence number to assign
    next_sequence: u64,
    /// Buffer for out-of-order results
    pending_results: BTreeMap<u64, CompressionResult>,
    /// Next sequence number expected for output
    next_output_sequence: u64,
    /// Whether the pool has been shut down
    shutdown: bool,
}

impl CompressionPool {
    /// Create a new compression pool with the specified number of worker threads
    pub fn new(num_threads: usize) -> Result<Self> {
        if num_threads == 0 {
            return Err(Error::InvalidInput("Thread count must be greater than 0".to_string()));
        }

        let (job_sender, job_receiver) = mpsc::channel();
        let (result_sender, result_receiver) = mpsc::channel();

        // Shared receiver for jobs - workers will compete for jobs
        let shared_job_receiver = Arc::new(Mutex::new(job_receiver));
        let mut workers = Vec::new();

        // Spawn worker threads
        for worker_id in 0..num_threads {
            let job_receiver = Arc::clone(&shared_job_receiver);
            let result_sender = result_sender.clone();

            let worker = thread::spawn(move || {
                Self::worker_loop(worker_id, job_receiver, result_sender);
            });

            workers.push(worker);
        }

        // Drop the original result sender so only workers hold copies
        drop(result_sender);

        Ok(CompressionPool {
            job_sender: Some(job_sender),
            result_receiver,
            workers,
            next_sequence: 0,
            pending_results: BTreeMap::new(),
            next_output_sequence: 0,
            shutdown: false,
        })
    }

    /// Submit a block for compression
    ///
    /// Returns the sequence number assigned to this job
    pub fn compress_block(&mut self, data: Vec<u8>, level: i32) -> Result<u64> {
        if self.shutdown {
            return Err(Error::InvalidInput("Compression pool is shut down".to_string()));
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;

        let job = CompressionJob {
            sequence,
            data,
            level,
        };

        if let Some(ref sender) = self.job_sender {
            sender.send(job).map_err(|_| {
                Error::InvalidInput("Failed to send job to workers".to_string())
            })?;
        } else {
            return Err(Error::InvalidInput("Compression pool is shut down".to_string()));
        }

        Ok(sequence)
    }

    /// Get the next completed compression result in sequence order
    ///
    /// Returns None if no results are ready or if the pool is empty and shut down
    /// Returns (compressed_data, original_data)
    pub fn get_result(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        // Check if we have the next expected result in pending results
        if let Some(result) = self.pending_results.remove(&self.next_output_sequence) {
            self.next_output_sequence += 1;
            return match result.error {
                Some(err) => Err(err),
                None => Ok(Some((result.compressed, result.original_data))),
            };
        }

        // Try to receive new results from workers
        loop {
            match self.result_receiver.try_recv() {
                Ok(result) => {
                    if result.sequence == self.next_output_sequence {
                        // This is the next result we're waiting for
                        self.next_output_sequence += 1;
                        return match result.error {
                            Some(err) => Err(err),
                            None => Ok(Some((result.compressed, result.original_data))),
                        };
                    } else {
                        // Store for later - out of order result
                        self.pending_results.insert(result.sequence, result);
                    }
                },
                Err(mpsc::TryRecvError::Empty) => {
                    // No results ready
                    return Ok(None);
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Workers have finished - check if we have pending results
                    if let Some(result) = self.pending_results.remove(&self.next_output_sequence) {
                        self.next_output_sequence += 1;
                        return match result.error {
                            Some(err) => Err(err),
                            None => Ok(Some((result.compressed, result.original_data))),
                        };
                    }
                    return Ok(None);
                }
            }
        }
    }

    /// Wait for all pending jobs to complete and return results in order
    /// Returns (compressed_data, original_data) pairs
    pub fn finish(&mut self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if !self.shutdown {
            // Signal workers to shut down by dropping the job sender
            self.job_sender.take();
            self.shutdown = true;
        }

        let mut results = Vec::new();

        // Collect all remaining results using blocking recv instead of try_recv
        loop {
            match self.result_receiver.recv() {
                Ok(result) => {
                    if result.sequence == self.next_output_sequence {
                        // This is the next result we're waiting for
                        self.next_output_sequence += 1;
                        match result.error {
                            Some(err) => return Err(err),
                            None => results.push((result.compressed, result.original_data)),
                        }
                    } else {
                        // Store for later - out of order result
                        self.pending_results.insert(result.sequence, result);
                    }

                    // Check for any pending results that are now ready
                    while let Some(pending_result) = self.pending_results.remove(&self.next_output_sequence) {
                        self.next_output_sequence += 1;
                        match pending_result.error {
                            Some(err) => return Err(err),
                            None => results.push((pending_result.compressed, pending_result.original_data)),
                        }
                    }
                },
                Err(_) => {
                    // Channel closed, no more results
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Shut down the pool and wait for all workers to finish
    pub fn shutdown(&mut self) -> Result<()> {
        if !self.shutdown {
            // Signal workers to shut down by dropping the job sender
            self.job_sender.take();
            self.shutdown = true;

            // Wait for all workers to finish
            let workers = std::mem::take(&mut self.workers);
            for worker in workers {
                worker.join().map_err(|_| {
                    Error::InvalidInput("Failed to join worker thread".to_string())
                })?;
            }
        }

        Ok(())
    }

    /// Worker thread main loop
    fn worker_loop(
        _worker_id: usize,
        job_receiver: Arc<Mutex<Receiver<CompressionJob>>>,
        result_sender: Sender<CompressionResult>,
    ) {
        loop {
            // Try to get a job
            let job = {
                let receiver = job_receiver.lock().unwrap();
                match receiver.recv() {
                    Ok(job) => job,
                    Err(_) => break, // Channel closed, shut down
                }
            };

            // Compress the data
            let mut compressed = Vec::new();
            let compression_result = encode(&mut compressed, &job.data, job.level);

            let result = CompressionResult {
                sequence: job.sequence,
                compressed,
                original_data: job.data.clone(),
                error: compression_result.err(),
            };

            // Send result back
            if result_sender.send(result).is_err() {
                // Result receiver was dropped, shut down
                break;
            }
        }
    }
}

impl Drop for CompressionPool {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Buffer pool for efficient memory reuse in parallel compression
pub struct BufferPool {
    /// Pool of available buffers
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Default buffer capacity
    default_capacity: usize,
}

impl BufferPool {
    /// Create a new buffer pool
    pub fn new(default_capacity: usize) -> Self {
        BufferPool {
            pool: Arc::new(Mutex::new(Vec::new())),
            default_capacity,
        }
    }

    /// Get a buffer from the pool or create a new one
    pub fn get_buffer(&self) -> Vec<u8> {
        let mut pool = self.pool.lock().unwrap();
        pool.pop().unwrap_or_else(|| Vec::with_capacity(self.default_capacity))
    }

    /// Return a buffer to the pool
    pub fn return_buffer(&self, mut buffer: Vec<u8>) {
        buffer.clear(); // Clear contents but keep capacity
        let mut pool = self.pool.lock().unwrap();

        // Limit pool size to prevent unbounded growth
        if pool.len() < 32 {
            pool.push(buffer);
        }
    }

    /// Get current pool size (for debugging)
    pub fn pool_size(&self) -> usize {
        self.pool.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_pool_basic() {
        let mut pool = CompressionPool::new(2).unwrap();

        // Submit some jobs
        let data1 = b"Hello, world! This is test data.".to_vec();
        let data2 = b"More test data for compression.".to_vec();

        let seq1 = pool.compress_block(data1.clone(), 1).unwrap();
        let seq2 = pool.compress_block(data2.clone(), 1).unwrap();

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);

        // Get results in order
        let results = pool.finish().unwrap();
        assert_eq!(results.len(), 2);

        // Results should be non-empty (compressed data)
        assert!(!results[0].0.is_empty());
        assert!(!results[1].0.is_empty());

        // Check original data
        assert_eq!(results[0].1, data1);
        assert_eq!(results[1].1, data2);
    }

    #[test]
    fn test_compression_pool_order() {
        let mut pool = CompressionPool::new(4).unwrap();

        // Submit multiple jobs quickly
        let mut sequences = Vec::new();
        for i in 0..10 {
            let data = format!("Test data {}", i).into_bytes();
            let seq = pool.compress_block(data, 1).unwrap();
            sequences.push(seq);
        }

        // Verify sequence numbers are in order
        for (i, &seq) in sequences.iter().enumerate() {
            assert_eq!(seq, i as u64);
        }

        // Get all results
        let results = pool.finish().unwrap();
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_buffer_pool() {
        let pool = BufferPool::new(1024);

        // Get a buffer
        let mut buffer1 = pool.get_buffer();
        assert!(buffer1.capacity() >= 1024);

        // Use the buffer
        buffer1.extend_from_slice(b"test data");
        assert_eq!(buffer1.len(), 9);

        // Return it
        pool.return_buffer(buffer1);
        assert_eq!(pool.pool_size(), 1);

        // Get another buffer - should reuse the previous one
        let buffer2 = pool.get_buffer();
        assert!(buffer2.is_empty()); // Should be cleared
        assert!(buffer2.capacity() >= 1024);
    }

    #[test]
    fn test_compression_pool_single_thread() {
        let mut pool = CompressionPool::new(1).unwrap();

        let data = b"Single threaded test data".to_vec();
        let data_len = data.len();
        pool.compress_block(data, 2).unwrap();

        let results = pool.finish().unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].0.is_empty());
        assert_eq!(results[0].1.len(), data_len);
    }

    #[test]
    fn test_compression_pool_empty() {
        let mut pool = CompressionPool::new(2).unwrap();

        // No jobs submitted
        let results = pool.finish().unwrap();
        assert!(results.is_empty());
    }
}