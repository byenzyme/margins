// Quick standalone test: verify the recorder starts and produces non-zero peaks
use margins::recorder;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    println!("=== Testing audio peak levels ===");

    // List devices first
    let devices = recorder::list_input_devices();
    println!("Found {} input devices:", devices.len());
    for (name, _) in &devices {
        println!("  - {}", name);
    }

    let default = recorder::default_input_device_name();
    println!("Default: {:?}", default);

    // Start recording
    let stop_flag = Arc::new(AtomicBool::new(false));
    let handle = match recorder::RecorderHandle::start(stop_flag.clone(), None) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to start recorder: {}", e);
            return;
        }
    };

    let mic_peak = handle.mic_peak();
    let spk_peak = handle.spk_peak();

    // Forward peaks through a shared atomic (same as desktop app)
    let shared_mic = Arc::new(AtomicU32::new(0));
    let shared_spk = Arc::new(AtomicU32::new(0));

    let sm = shared_mic.clone();
    let ss = shared_spk.clone();
    let mp = mic_peak.clone();
    let sp = spk_peak.clone();
    let sf = stop_flag.clone();

    let forwarder = std::thread::spawn(move || {
        while !sf.load(Ordering::Relaxed) {
            let m = mp.swap(0, Ordering::Relaxed);
            let s = sp.swap(0, Ordering::Relaxed);
            sm.store(m, Ordering::Relaxed);
            ss.store(s, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(100));
        }
    });

    // Poll for 3 seconds, print peaks
    let mut any_nonzero = false;
    for i in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        let m = f32::from_bits(shared_mic.load(Ordering::Relaxed));
        let s = f32::from_bits(shared_spk.load(Ordering::Relaxed));
        let m_db = if m > 0.0 { 20.0 * m.log10() } else { -99.0 };
        let s_db = if s > 0.0 { 20.0 * s.log10() } else { -99.0 };
        println!(
            "[{:.1}s] mic: {:.6} ({:.1} dB)  spk: {:.6} ({:.1} dB)",
            (i + 1) as f64 * 0.25,
            m,
            m_db,
            s,
            s_db
        );
        if m > 0.0 || s > 0.0 {
            any_nonzero = true;
        }
    }

    stop_flag.store(true, Ordering::SeqCst);
    forwarder.join().unwrap();

    // Don't bother writing the WAV
    drop(handle);

    if any_nonzero {
        println!("\n=== PASS: Got non-zero audio levels ===");
    } else {
        println!("\n=== WARN: All levels were zero — check microphone permissions ===");
    }
}
