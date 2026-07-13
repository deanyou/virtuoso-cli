//! Async runtime support for tokio-powered I/O.
//!
//! This module provides the tokio runtime singleton and async wrappers
//! for blocking I/O operations (TCP, process execution).

use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// Global tokio runtime — initialized once at startup.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Get or create the global tokio runtime.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
    })
}

/// Get or create a multi-threaded tokio runtime for heavy concurrency.
#[allow(dead_code)]
pub fn multi_threaded_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(num_cpus::get())
            .build()
            .expect("failed to build multi-threaded tokio runtime")
    })
}

/// Run a blocking operation in the tokio thread pool.
#[allow(dead_code)]
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    runtime().block_on(future)
}
