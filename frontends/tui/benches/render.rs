//! Offline rendering measurement; no terminal, daemon, or benchmark dependencies.
use mypowers_tui::{
    app::{App, Trend, View},
    clock::Clock,
    model::Status,
    network::Event,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

const FRAMES: u32 = 5_000;
const ROUNDS: usize = 5;
const ALLOCATION_FRAMES: usize = 100;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// Safety: every allocation operation is forwarded to System with the original arguments.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

fn fixture(view: View, idle: bool) -> App {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let mut app = App::with_clock(
        false,
        Some(chrono_tz::UTC),
        Clock::Fixed {
            now,
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::from_secs(10),
            feedback_elapsed: Duration::from_secs(1),
        },
    );
    let status: Status = serde_json::from_value(json!({
        "schema_version": 1, "server_time": now.to_rfc3339(),
        "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10", "state_version": 1,
        "device": {"name": "AP S300 V2.0"},
        "connection": {"phase": "connected", "desired": "running", "link_connected": true,
            "session_id": "benchmark", "message": "Connected", "adapter_id": "hci2"},
        "telemetry": {"state": "live", "age_seconds": 0.4, "sample": {
            "sequence": 1, "received_at": now.to_rfc3339(), "segment_id": "benchmark",
            "battery_percent": 71, "input_power_w": if idle {0} else {63},
            "output_power_w": if idle {0} else {181}, "remaining_minutes": 2937,
            "ac_enabled": true, "dc_enabled": false, "light_enabled": false}},
        "controls": {"allowed": true, "outputs_revision": 1, "pending_command_id": null, "reason_code": null},
        "history": {"state": "ok"}, "logging": {"effective_level": "INFO", "override_expires_at": null}
    })).unwrap();
    let sample = status.telemetry.sample.as_ref().unwrap().clone();
    app.update(Event::Status(Box::new(status)));
    app.samples = (0..40)
        .map(|index| Trend {
            timestamp: now.timestamp() as f64 - f64::from(39 - index) * 3.0,
            sample: sample.clone(),
        })
        .collect();
    app.view = view;
    for sequence in 0..2_000 {
        app.logs.records.push_back(json!({
            "timestamp": now.to_rfc3339(), "level": "INFO", "sequence": sequence,
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "message": "Output operation completed."
        }));
    }
    app.logs.offset = 1_000;
    app
}

fn main() {
    for (name, view, idle) in [
        ("dashboard-live", View::Dashboard, false),
        ("dashboard-idle", View::Dashboard, true),
        ("logs-modal", View::Logs, false),
    ] {
        let mut app = fixture(view, idle);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        for _ in 0..100 {
            terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        }
        ALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        for _ in 0..ALLOCATION_FRAMES {
            terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        }
        COUNTING.store(false, Ordering::Relaxed);
        let allocations = ALLOCATIONS.load(Ordering::Relaxed) / ALLOCATION_FRAMES;
        let mut measurements = [Duration::ZERO; ROUNDS];
        for elapsed in &mut measurements {
            let start = Instant::now();
            for _ in 0..FRAMES {
                terminal
                    .draw(|frame| ui::draw(frame, black_box(&mut app)))
                    .unwrap();
                black_box(terminal.backend().buffer());
            }
            *elapsed = start.elapsed() / FRAMES;
        }
        measurements.sort();
        println!(
            "{name}: median {:.2} us/frame (min {:.2}, max {:.2}; {ROUNDS} × {FRAMES} frames); {allocations} allocations/reallocations per frame",
            measurements[ROUNDS / 2].as_secs_f64() * 1e6,
            measurements[0].as_secs_f64() * 1e6,
            measurements[ROUNDS - 1].as_secs_f64() * 1e6
        );
    }
}
