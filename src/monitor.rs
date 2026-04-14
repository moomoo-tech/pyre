//! Passive GIL contention monitor + decoupled RSS sampler.
//!
//! Previous design used an active watchdog thread that acquired the GIL every
//! 10ms to probe contention. This caused two problems:
//!
//! 1. **Observer effect**: the probe itself competes for the GIL, creating
//!    artificial contention and context switches (~5-10% throughput loss under
//!    heavy Python workloads).
//! 2. **Shutdown segfault**: the detached watchdog thread could outlive
//!    Py_Finalize, causing use-after-free on the global interpreter state.
//!
//! New design (Haskell bracket-inspired):
//! - **GIL metrics** are collected passively on the real request path — each
//!   `call_handler_with_hooks` records GIL acquisition wait time as a
//!   byproduct. Zero overhead when idle, zero artificial contention.
//! - **RSS sampling** runs in a separate non-GIL thread with an explicit
//!   stop flag and JoinHandle for deterministic shutdown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_utils::CachePadded;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// GIL contention metrics (updated passively by request handlers)
// ---------------------------------------------------------------------------

/// Last GIL acquisition wait time (microseconds)
pub static GIL_LATENCY_LAST_US: AtomicU64 = AtomicU64::new(0);
/// Peak GIL acquisition wait time since last reset (microseconds)
pub static GIL_LATENCY_MAX_US: AtomicU64 = AtomicU64::new(0);
/// Total probe count (= total handler invocations that acquired GIL)
pub static GIL_PROBE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total accumulated GIL wait (microseconds)
pub static GIL_TOTAL_WAIT_US: AtomicU64 = AtomicU64::new(0);

/// Memory RSS in bytes (updated by background sampler)
pub static MEMORY_RSS_BYTES: AtomicU64 = AtomicU64::new(0);

/// Number of threads currently waiting to acquire the main GIL
pub static GIL_QUEUE_LENGTH: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);
/// Peak business handler GIL hold time (microseconds, reset on read)
pub static GIL_HOLD_MAX_US: AtomicU64 = AtomicU64::new(0);

// Hot-path counters: CachePadded to avoid false sharing across CPU cores.
// Each counter gets its own 64-byte cache line.

/// Requests dropped due to backpressure (503 overloaded)
pub static DROPPED_REQUESTS: CachePadded<AtomicU64> = CachePadded::new(AtomicU64::new(0));
/// Total requests processed
pub static TOTAL_REQUESTS: CachePadded<AtomicU64> = CachePadded::new(AtomicU64::new(0));

// ---------------------------------------------------------------------------
// Passive GIL measurement (called from handlers.rs)
// ---------------------------------------------------------------------------

/// Record a GIL acquisition wait time. Called from `call_handler_with_hooks`
/// immediately after `Python::attach` succeeds.
///
/// This replaces the active watchdog probe — measures real request latency
/// instead of artificial contention from a background thread.
#[inline]
pub fn record_gil_wait(wait_us: u64) {
    GIL_LATENCY_LAST_US.store(wait_us, Ordering::Relaxed);
    GIL_LATENCY_MAX_US.fetch_max(wait_us, Ordering::Relaxed);
    GIL_TOTAL_WAIT_US.fetch_add(wait_us, Ordering::Relaxed);
    GIL_PROBE_COUNT.fetch_add(1, Ordering::Relaxed);

    if wait_us > 50_000 {
        tracing::warn!(
            target: "pyre::server",
            latency_ms = wait_us / 1000,
            "GIL congested (measured on real request)"
        );
    }
}

// ---------------------------------------------------------------------------
// Decoupled RSS sampler (no GIL, deterministic shutdown)
// ---------------------------------------------------------------------------

/// Stop flag for the RSS sampler thread.
static RSS_SAMPLER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Spawn a lightweight background thread that samples process RSS.
/// Returns a JoinHandle for deterministic shutdown (caller must join).
///
/// This thread never touches Python or the GIL — it only reads /proc/self/statm.
pub fn spawn_rss_sampler() -> std::thread::JoinHandle<()> {
    RSS_SAMPLER_RUNNING.store(true, Ordering::Release);
    std::thread::Builder::new()
        .name("pyre-rss-sampler".to_string())
        .spawn(|| {
            while RSS_SAMPLER_RUNNING.load(Ordering::Relaxed) {
                MEMORY_RSS_BYTES.store(get_rss_bytes(), Ordering::Relaxed);
                // 1 second interval — RSS doesn't change fast enough to warrant
                // more frequent sampling, and this thread does zero GIL work.
                std::thread::sleep(Duration::from_secs(1));
            }
            tracing::debug!(target: "pyre::server", "RSS sampler stopped");
        })
        .expect("failed to spawn RSS sampler")
}

/// Signal the RSS sampler to stop. Non-blocking — call join() on the
/// returned JoinHandle to wait for actual termination.
pub fn stop_rss_sampler() {
    RSS_SAMPLER_RUNNING.store(false, Ordering::Release);
}

/// Get current process RSS in bytes (platform-specific, zero dependencies).
fn get_rss_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        use std::mem;
        unsafe {
            let mut info: libc_mach_task_basic_info = mem::zeroed();
            let mut count = (mem::size_of::<libc_mach_task_basic_info>() / 4) as u32;
            let kr = mach_task_self_info(&mut info, &mut count);
            if kr == 0 {
                return info.resident_size;
            }
        }
        0
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|pages| pages * 4096)
            .unwrap_or(0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

// macOS: minimal FFI for task_info (avoids libc crate dependency)
#[cfg(target_os = "macos")]
#[repr(C)]
struct libc_mach_task_basic_info {
    virtual_size: u64,
    resident_size: u64,
    resident_size_max: u64,
    user_time: [u32; 2],
    system_time: [u32; 2],
    policy: i32,
    suspend_count: i32,
}

#[cfg(target_os = "macos")]
unsafe fn mach_task_self_info(info: &mut libc_mach_task_basic_info, count: &mut u32) -> i32 {
    extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut u8, count: *mut u32) -> i32;
    }
    // MACH_TASK_BASIC_INFO = 20
    task_info(mach_task_self(), 20, info as *mut _ as *mut u8, count)
}

// ---------------------------------------------------------------------------
// Python-facing metrics API
// ---------------------------------------------------------------------------

/// Get all metrics. Returns tuple:
/// (last_us, peak_us, probe_count, total_wait_us, rss_bytes,
///  queue_len, hold_peak_us, dropped_requests, total_requests)
/// Resets peaks after read.
#[pyfunction]
pub fn get_gil_metrics() -> (u64, u64, u64, u64, u64, isize, u64, u64, u64) {
    let last = GIL_LATENCY_LAST_US.load(Ordering::Relaxed);
    let peak = GIL_LATENCY_MAX_US.swap(0, Ordering::Relaxed);
    let count = GIL_PROBE_COUNT.load(Ordering::Relaxed);
    let total = GIL_TOTAL_WAIT_US.load(Ordering::Relaxed);
    let rss = MEMORY_RSS_BYTES.load(Ordering::Relaxed);
    let queue = GIL_QUEUE_LENGTH.load(std::sync::atomic::Ordering::Relaxed);
    let hold_peak = GIL_HOLD_MAX_US.swap(0, Ordering::Relaxed);
    let dropped = DROPPED_REQUESTS.load(Ordering::Relaxed);
    let total_req = TOTAL_REQUESTS.load(Ordering::Relaxed);
    (
        last, peak, count, total, rss, queue, hold_peak, dropped, total_req,
    )
}
