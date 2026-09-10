//! Integration test: simulates the desktop app's recording flow.
//! Verifies that start_recording → get_recording_status → stop_recording
//! produces non-zero mic peak levels through the shared atomic chain.

use chrono::Local;
use margins::{recorder, session};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let work_dir = std::env::temp_dir().join("margins-test-desktop");
    let margins_dir = work_dir.join(".margins");
    let _ = std::fs::create_dir_all(&margins_dir);

    let session_name = format!("test-rec-{}", std::process::id());
    let start_time = Local::now();
    let notes_path = format!("{}.md", session_name);

    // Create session (same as desktop app's start_recording command)
    session::create_session(&margins_dir, &session_name, &start_time, &notes_path).unwrap();
    let wav_path = format!(".margins/{}_seg0.wav", session_name);
    session::add_segment(&margins_dir, &session_name, 0, &wav_path, 0, None).unwrap();

    // Shared atomics (same as desktop RecordingState)
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mic_peak = Arc::new(AtomicU32::new(0));
    let spk_peak = Arc::new(AtomicU32::new(0));

    let stop_clone = stop_flag.clone();
    let mic_clone = mic_peak.clone();
    let spk_clone = spk_peak.clone();
    let wav_full = work_dir.join(&wav_path).to_string_lossy().to_string();

    // Spawn recorder thread (same as desktop app)
    let thread = std::thread::spawn(move || -> Result<f64, String> {
        let handle =
            recorder::RecorderHandle::start(stop_clone.clone(), None).map_err(|e| e.to_string())?;

        let rec_mic = handle.mic_peak();
        let rec_spk = handle.spk_peak();

        while !stop_clone.load(Ordering::Relaxed) {
            let m = rec_mic.swap(0, Ordering::Relaxed);
            let s = rec_spk.swap(0, Ordering::Relaxed);
            mic_clone.store(m, Ordering::Relaxed);
            spk_clone.store(s, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(100));
        }

        handle.stop_and_write(&wav_full).map_err(|e| e.to_string())
    });

    // Simulate frontend polling (same as get_recording_status)
    println!("=== Simulating desktop recording flow ===");
    let mut saw_mic = false;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        let mic = f32::from_bits(mic_peak.load(Ordering::Relaxed));
        let spk = f32::from_bits(spk_peak.load(Ordering::Relaxed));
        let elapsed = (Local::now() - start_time).num_milliseconds() as f64 / 1000.0;
        println!(
            "[{:.1}s] mic_level={:.6} spk_level={:.6}",
            elapsed, mic, spk
        );
        if mic > 0.0 {
            saw_mic = true;
        }
    }

    // Stop
    stop_flag.store(true, Ordering::SeqCst);
    let duration = thread.join().unwrap().unwrap();
    session::update_segment_duration(&margins_dir, &session_name, 0, duration).unwrap();
    println!("\nRecorded {:.1}s", duration);

    // Verify
    if saw_mic {
        println!("=== PASS: mic levels visible through shared atomic chain ===");
    } else {
        println!("=== FAIL: mic levels were always zero ===");
        std::process::exit(1);
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&work_dir);
}
