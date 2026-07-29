use std::io::Write;

use serde_json::Value;

/// Narrow a recording to its owner.
///
/// It holds every command and every response of the session, which includes the values a
/// fill put into the page — among them the ones redacted on stdout precisely because they
/// are secrets. It was created with whatever the umask allowed, typically 0644, while
/// screenshot, pdf, download and the session store all chmod 0600. Applied on every write
/// rather than at creation: the file may already exist, wider, from an earlier run.
fn restrict(path: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Open (or create) a recording file for append and write nothing yet.
/// Returns an error if the file cannot be opened.
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
