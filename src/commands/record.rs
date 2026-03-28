use std::io::Write;

use serde_json::Value;

/// Open (or create) a recording file for append and write nothing yet.
/// Returns an error if the file cannot be opened.
pub fn start_recording(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open recording file '{path}': {e}"))?;
    Ok(())
}

/// Append a `{"cmd": ..., "response": ...}` JSON line to the recording file.
pub fn log_entry(path: &str, cmd: &Value, response: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open recording file '{path}': {e}"))?;

    let entry = serde_json::json!({
        "cmd": cmd,
        "response": response,
    });
    let line = serde_json::to_string(&entry)?;
    writeln!(file, "{line}")?;
    Ok(())
}
