use std::io::Write;

use serde_json::Value;

/// chmod 0600: a recording holds every command and response, including values redacted on
/// stdout because they are secrets. Applied on every write, since the file may already exist
/// with wider permissions from an earlier run.
fn restrict(path: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Open or create the recording file for append, writing nothing. Errors when it cannot be
/// opened, which refuses the command rather than running it unrecorded.
pub fn start_recording(path: &str) -> Result<(), crate::BoxError> {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open recording file '{path}': {e}"))?;
    restrict(path);
    Ok(())
}

/// Append a `{"cmd": ..., "response": ...}` JSON line to the recording file.
pub fn log_entry(path: &str, cmd: &Value, response: &Value) -> Result<(), crate::BoxError> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Failed to open recording file '{path}': {e}"))?;
    restrict(path);

    let entry = serde_json::json!({
        "cmd": cmd,
        "response": response,
    });
    let line = serde_json::to_string(&entry)?;
    writeln!(file, "{line}")?;
    Ok(())
}
