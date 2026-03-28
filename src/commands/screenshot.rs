use std::path::PathBuf;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{CaptureScreenshotParams, CaptureScreenshotResult};

pub async fn run(
    client: &CdpClient,
    filename: Option<&str>,
) -> Result<String, crate::BoxError> {
    let result: CaptureScreenshotResult = client
        .call(
            "Page.captureScreenshot",
            CaptureScreenshotParams {
                format: Some("png".into()),
                quality: None,
                clip: None,
                from_surface: None,
                capture_beyond_viewport: None,
                optimize_for_speed: None,
            },
        )
        .await?;

    // Decode base64 image data
    let bytes = base64_decode(&result.data)?;

    // Determine output path
    let dir = screenshot_dir()?;
    std::fs::create_dir_all(&dir)?;

    let file_name = if let Some(name) = filename {
        // Sanitize: extract only the filename component, strip path traversal
        let sanitized = std::path::Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("screenshot");
        if sanitized.ends_with(".png") {
            sanitized.to_string()
        } else {
            format!("{sanitized}.png")
        }
    } else {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("screenshot-{ts}.png")
    };

    let path = dir.join(&file_name);
    std::fs::write(&path, &bytes)?;

    // Restrict permissions (readable only by owner)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(path.display().to_string())
}

fn screenshot_dir() -> Result<PathBuf, crate::BoxError> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".aibrowsr").join("tmp"))
}

/// Minimal base64 decoder (RFC 4648). Avoids pulling in the `base64` crate.
fn base64_decode(input: &str) -> Result<Vec<u8>, crate::BoxError> {
    // Compile-time lookup table (Rust 2024 const block)
    const LOOKUP: [u8; 256] = const {
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut lut = [255u8; 256];
        let mut i = 0;
        while i < 64 {
            lut[table[i] as usize] = i as u8;
            i += 1;
        }
        lut
    };

    let input = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &b in input {
        if matches!(b, b'=' | b'\n' | b'\r' | b' ') {
            continue;
        }
        let val = LOOKUP[b as usize];
        if val == 255 {
            return Err(format!("Invalid base64 character: {}", b as char).into());
        }
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}
