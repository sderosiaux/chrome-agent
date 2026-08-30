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
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DaemonError(format!("Failed to create socket dir: {e}")))?;
    }

    if let Ok(pid_path) = session::daemon_pid_path() {
        let _ = std::fs::write(&pid_path, format!("{}\n", std::process::id()));
    }

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| DaemonError(format!("Failed to bind {}: {e}", socket_path.display())))?;

    eprintln!("daemon ready on {}", socket_path.display());

    let (activity_tx, mut activity_rx) = mpsc::channel::<()>(16);
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let mut last_activity = Instant::now();

    // Heartbeat task: check Chrome health periodically. It must NOT touch `activity_tx` —
    // only client traffic resets the idle timer, or the 2s beat keeps the daemon alive
    // forever and IDLE_TIMEOUT is never reached.
    let _heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        loop {
            interval.tick().await;
            // Load the session and verify browser pids. A store that will not load is
            // reported on stderr rather than skipped silently: the beat has no client to
            // answer, so saying so is the only remedy available.
            let mut store = match session::load_session() {
                Ok(store) => store,
                Err(e) => {
                    eprintln!("daemon heartbeat: could not read the session store: {e}");
                    continue;
                }
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
        // Client traffic, and only client traffic, resets the idle timer.
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
            // Break the main loop so its cleanup (socket + pid removal) runs. Never
            // std::process::exit here: that leaks the socket file.
            let _ = shutdown.send(()).await;
            break;
        }
    }
}

/// Process a daemon command: the JSON response, plus whether the daemon should shut down
/// after replying (only `stop` sets it).
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
            // A store that will not load answers an empty browser list — there is nothing
            // truthful to put in it — and the reason goes to stderr.
            let store = session::load_session().unwrap_or_else(|e| {
                eprintln!("daemon status: could not read the session store: {e}");
                session::SessionStore::default()
            });
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
        // `stop` must set the shutdown flag so the main loop breaks and cleans up,
        // rather than calling std::process::exit.
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
        // The heartbeat fires far more often than the daemon idles, so feeding the
        // activity channel from it would keep the daemon alive forever.
        assert!(
            HEARTBEAT_INTERVAL < IDLE_TIMEOUT,
            "heartbeat must be shorter than the idle timeout — otherwise resetting \
             activity on every beat can never let the daemon idle out"
        );
    }
}
