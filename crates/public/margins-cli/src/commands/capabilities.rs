use crate::error::CliError;
use std::io::Write;

pub fn public(stdout: &mut dyn Write) -> Result<(), CliError> {
    let mut distillation_inputs = vec!["transcript", "memo"];
    if cfg!(any(feature = "coreml-asr", feature = "parakeet-onnx")) {
        distillation_inputs.push("audio");
    }
    let report = serde_json::json!({
        "schema": 1,
        "product": "margins",
        "composition": "public",
        "build": crate::build_info::get(),
        "workspace": {
            "setup": true,
            "program": true,
            "declarations": ["workspace", "source"],
            "lifecycle": ["init", "sync"],
            "automation": ["plan", "apply"],
        },
        "recall": {
            "available": true,
            "lookup": true,
            "mode": "live_local_markdown",
        },
        "distillation": {
            "available": true,
            "workflow": "connected_note",
            "inputs": distillation_inputs,
        },
    });
    writeln!(stdout, "{report}").map_err(|error| CliError::from_anyhow(error.into()))
}
