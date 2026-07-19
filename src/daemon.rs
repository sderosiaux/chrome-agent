use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::session;

const IDLE_TIMEOUT: Duration = Duration::from_mins(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// Run the micro-daemon. Blocks until idle timeout or explicit stop.
pub async fn run_daemon(socket_path: &Path) -> Result<(), DaemonError> {
    // Clean up stale socket
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DaemonError(format!("Failed to create socket dir: {e}")))?;
    }

    // Write PID file
    if let Ok(pid_path) = session::daemon_pid_path() {
        let _ = std::fs::write(&pid_path, format!("{}\n", std::process::id()));
    }

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| DaemonError(format!("Failed to bind {}: {e}", socket_path.display())))?;

    eprintln!("daemon ready on {}", socket_path.display());

    let (activity_tx, mut activity_rx) = mpsc::channel::<()>(16);
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let mut last_activity = Instant::now();

    // Heartbeat task: check Chrome health periodically. It deliberately does NOT
    // touch `activity_tx` — only real client traffic must reset the idle timer,
    // otherwise the 2s heartbeat would keep the daemon alive forever and the
    // IDLE_TIMEOUT would never be reached.
    let _heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        loop {
            interval.tick().await;
            // Heartbeat logic: try to load session and verify browser PIDs
            let Ok(mut store) = session::load_session() else {
                continue;
            };
            let before = store.browsers.len();
            session::cleanup_stale(&mut store);
            if store.browsers.len() != before {
                let _ = session::save_session(&mut store);
            }
        }
    });

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        last_activity = Instant::now();
                        let tx = activity_tx.clone();
                        let stop = shutdown_tx.clone();
                        tokio::spawn(handle_client(stream, tx, stop));
                    }
                    Err(e) => {
                        eprintln!("daemon accept error: {e}");
                    }
                }
            }

            _ = activity_rx.recv() => {
                last_activity = Instant::now();
            }

            _ = shutdown_rx.recv() => {
                eprintln!("daemon received stop, exiting");
                break;
            }

            () = tokio::time::sleep(IDLE_TIMEOUT.saturating_sub(last_activity.elapsed())) => {
                if last_activity.elapsed() >= IDLE_TIMEOUT {
                    eprintln!("daemon idle timeout, exiting");
                    break;
                }
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(socket_path);
    if let Ok(pid_path) = session::daemon_pid_path() {
        let _ = std::fs::remove_file(&pid_path);
    }

    Ok(())
}

/// Handle a single client connection. Protocol: newline-delimited JSON.
/// Request: `{"command": "...", "args": {...}}`
/// Response: `{"ok": true, "data": ...}` or `{"ok": false, "error": "..."}`
async fn handle_client(
    stream: UnixStream,
    activity: mpsc::Sender<()>,
    shutdown: mpsc::Sender<()>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        // Real client traffic resets the daemon's idle timer.
        let _ = activity.send(()).await;

        let (response, should_shutdown) = process_command(&line);
        let json = serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"ok":false,"error":"serialization failed"}"#.to_string()
        });
        // Write the response before triggering shutdown so the client sees it.
        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
            break;
        }
        if should_shutdown {
            // Signal the main loop to break so its cleanup (socket + PID removal)
            // runs. Do NOT std::process::exit here — that leaks the socket file.
            let _ = shutdown.send(()).await;
            break;
        }
    }
}

/// Process a daemon command. Thin dispatch layer.
///
/// Returns the JSON response plus a flag indicating whether the daemon should
/// shut down after replying (only set by the `stop` command).
fn process_command(line: &str) -> (serde_json::Value, bool) {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return (
                serde_json::json!({"ok": false, "error": format!("Invalid JSON: {e}")}),
                false,
            );
        }
    };

    let command = request
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match command {
        "ping" => (serde_json::json!({"ok": true, "data": "pong"}), false),

        "status" => {
            let store = session::load_session().unwrap_or_default();
            let browsers: Vec<&str> = store.browsers.keys().map(std::string::String::as_str).collect();
            (
                serde_json::json!({
                    "ok": true,
                    "data": {
                        "pid": std::process::id(),
                        "browsers": browsers,
                    }
                }),
                false,
            )
        }

        // Reply "stopping" and ask the main loop to break so cleanup runs.
        "stop" => (serde_json::json!({"ok": true, "data": "stopping"}), true),

        _ => (
            serde_json::json!({"ok": false, "error": format!("Unknown command: {command}")}),
            false,
        ),
    }
}

/// Start the daemon in a background process (fork on Unix).
#[allow(dead_code)] // Used in v2.1 daemon auto-start
pub fn spawn_daemon() -> Result<(), DaemonError> {
    let exe = std::env::current_exe()
        .map_err(|e| DaemonError(format!("Cannot find own executable: {e}")))?;

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon");
    cmd.arg("start");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid() creates a new session so the daemon doesn't die with the terminal.
        // No shared state is accessed. This is the standard Unix daemonization pattern.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    cmd.spawn()
        .map_err(|e| DaemonError(format!("Failed to spawn daemon: {e}")))?;

    Ok(())
}

/// Ensure daemon is running. Start it if not.
#[allow(dead_code)] // Used in v2.1 daemon auto-start
pub async fn ensure_daemon() -> Result<(), DaemonError> {
    if session::daemon_socket_exists() {
        // Try connecting to verify it's alive
        let socket_path = session::daemon_socket_path()
            .map_err(|e| DaemonError(e.to_string()))?;
        if try_ping_daemon(&socket_path).await {
            return Ok(());
        }
        // Socket exists but daemon is dead — clean up
        let _ = std::fs::remove_file(&socket_path);
    }

    spawn_daemon()?;

    // Wait for daemon to be ready
    let socket_path = session::daemon_socket_path()
        .map_err(|e| DaemonError(e.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if try_ping_daemon(&socket_path).await {
            return Ok(());
        }
    }

    Err(DaemonError("Daemon failed to start within 5s".into()))
}

/// Try to ping the daemon. Returns true if it responds.
#[allow(dead_code)] // Used in v2.1 daemon auto-start
async fn try_ping_daemon(socket_path: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path).await else {
        return false;
    };

    use tokio::io::AsyncReadExt;
    let msg = r#"{"command":"ping"}"#;
    if stream.write_all(format!("{msg}\n").as_bytes()).await.is_err() {
        return false;
    }
    if stream.shutdown().await.is_err() {
        return false;
    }

    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;
    let response = String::from_utf8_lossy(&buf);
    response.contains("pong")
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DaemonError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_does_not_request_shutdown() {
        let (resp, shutdown) = process_command(r#"{"command":"ping"}"#);
        assert!(!shutdown);
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"], "pong");
    }

    #[test]
    fn stop_requests_graceful_shutdown() {
        // Regression for A3b: `stop` must trigger the main-loop break (so socket +
        // PID cleanup runs), signalled by the shutdown flag — not std::process::exit.
        let (resp, shutdown) = process_command(r#"{"command":"stop"}"#);
        assert!(shutdown, "stop must request shutdown");
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"], "stopping");
    }

    #[test]
    fn invalid_json_reports_error_without_shutdown() {
        let (resp, shutdown) = process_command("not json");
        assert!(!shutdown);
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("Invalid JSON"));
    }

    #[test]
    fn unknown_command_reports_error_without_shutdown() {
        let (resp, shutdown) = process_command(r#"{"command":"frobnicate"}"#);
        assert!(!shutdown);
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("Unknown command"));
    }

    #[test]
    fn heartbeat_cannot_reset_idle_timer() {
        // Regression for A3a: the heartbeat fires far more often than the daemon
        // idles, so if it ever fed the activity channel the daemon would live
        // forever. Guard the invariant that keeps the fix meaningful.
        assert!(
            HEARTBEAT_INTERVAL < IDLE_TIMEOUT,
            "heartbeat must be shorter than the idle timeout — otherwise resetting \
             activity on every beat can never let the daemon idle out"
        );
    }
}
