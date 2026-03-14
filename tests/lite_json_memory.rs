//! Peak heap memory measurement during OMI message parsing.
//!
//! Uses a tracking global allocator to measure peak heap usage while parsing
//! representative OMI messages.  Run under either `json` or `lite-json` feature:
//!
//!   cargo test --target x86_64-unknown-linux-gnu --no-default-features \
//!     --features std,json --config unstable.build-std=[] \
//!     --test lite_json_memory -- --nocapture
//!
//! The companion script `scripts/measure-lite-json.sh` runs this automatically
//! under both feature sets and compares the results.

#![cfg(any(feature = "json", feature = "lite-json"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use reconfigurable_device::omi::OmiMessage;

// ── Tracking allocator ──────────────────────────────────────────────────────

struct TrackingAlloc;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let cur = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            // Update peak (compare-exchange loop for correctness)
            let mut peak = PEAK.load(Ordering::Relaxed);
            while cur > peak {
                match PEAK.compare_exchange_weak(peak, cur, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(p) => peak = p,
                }
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOC: TrackingAlloc = TrackingAlloc;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn reset_peak() {
    PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

fn current_bytes() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

/// Measure peak heap for parsing a single message.
/// Returns (peak_during_parse, retained_after_drop).
fn measure_parse(json: &str) -> (usize, usize) {
    // Warm up: parse once to trigger any lazy init
    let _ = OmiMessage::parse(json);

    let baseline = current_bytes();
    reset_peak();

    let msg = OmiMessage::parse(json).expect("parse failed");
    let peak = peak_bytes() - baseline;
    let retained = current_bytes() - baseline;

    drop(msg);
    let after_drop = current_bytes() - baseline;
    let _ = after_drop; // suppress unused warning

    (peak, retained)
}

// ── Test messages ───────────────────────────────────────────────────────────

const READ_MSG: &str = r#"{"omi":"1.0","ttl":10,"read":{"path":"/Objects/Thermostat/temperature"}}"#;

const WRITE_MSG: &str = r#"{"omi":"1.0","ttl":10,"write":{"path":"/Objects/Thermostat/setpoint","v":22.5}}"#;

const WRITE_BATCH_MSG: &str = r#"{"omi":"1.0","ttl":10,"write":{"items":[{"path":"/Sensor/temp","v":21.3,"t":1700000000.0},{"path":"/Sensor/humidity","v":55.2,"t":1700000001.0},{"path":"/Sensor/pressure","v":1013.25,"t":1700000002.0},{"path":"/Sensor/co2","v":412,"t":1700000003.0},{"path":"/Sensor/voc","v":0.5,"t":1700000004.0}]}}"#;

const RESPONSE_MSG: &str = r#"{"omi":"1.0","ttl":0,"response":{"status":200,"rid":"req-42","result":{"path":"/Objects/Thermostat/temperature","values":[{"v":21.5,"t":1700000000.0}]}}}"#;

const DELETE_MSG: &str = r#"{"omi":"1.0","ttl":10,"delete":{"path":"/Objects/OldSensor"}}"#;

const CANCEL_MSG: &str = r#"{"omi":"1.0","ttl":10,"cancel":{"rid":["sub-001","sub-002","sub-003"]}}"#;

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn measure_peak_memory() {
    let feature = if cfg!(feature = "lite-json") {
        "lite-json"
    } else {
        "json"
    };

    eprintln!();
    eprintln!("  Peak heap memory during OMI parsing (feature: {feature})");
    eprintln!("  ─────────────────────────────────────────────────────────");

    let messages: &[(&str, &str)] = &[
        ("read",        READ_MSG),
        ("write(2)",    WRITE_MSG),
        ("write(5)",    WRITE_BATCH_MSG),
        ("response",    RESPONSE_MSG),
        ("delete",      DELETE_MSG),
        ("cancel",      CANCEL_MSG),
    ];

    let mut total_peak = 0usize;
    let mut total_retained = 0usize;

    for (label, json) in messages {
        let (peak, retained) = measure_parse(json);
        total_peak += peak;
        total_retained += retained;
        eprintln!(
            "  {label:<12} peak: {peak:>6} B   retained: {retained:>6} B   input: {:>4} B",
            json.len()
        );
    }

    eprintln!("  ─────────────────────────────────────────────────────────");
    eprintln!(
        "  TOTAL        peak: {total_peak:>6} B   retained: {total_retained:>6} B"
    );
    eprintln!();

    // Output machine-readable line for the script to parse
    eprintln!("MEMORY_RESULT:{feature}:peak={total_peak}:retained={total_retained}");
}
