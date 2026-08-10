use margins_core::{AudioLane, CaptureErrorCode, CaptureProvider, UnavailableCaptureProvider};

fn main() {
    let provider = UnavailableCaptureProvider::default();
    let capabilities = provider.capabilities();
    assert!(!capabilities.available);

    let error = provider.permission(AudioLane::Microphone).unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::Unavailable);
    println!("{}", error.message);
}
