use std::time::Duration;

fn main() {
    match margins_capture::recorder::test_system_audio_tap_level(Duration::from_millis(1_800)) {
        Ok(probe) => {
            println!(
                "tap_opened=true peak={:.8} drop_count={} silent_secs={:.3} frame_count={} test_tone_played={}",
                probe.peak,
                probe.drop_count,
                probe.silent_secs,
                probe.frame_count,
                probe.test_tone_played
            );
        }
        Err(error) => {
            eprintln!("tap_opened=false error={error:#}");
            std::process::exit(2);
        }
    }
}
