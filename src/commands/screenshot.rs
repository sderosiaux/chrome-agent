use std::path::PathBuf;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{CaptureScreenshotParams, CaptureScreenshotResult};

pub async fn run(
    client: &CdpClient,
    filename: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
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

    let file_name = match filename {
        Some(name) => {
            if name.ends_with(".png") {
                name.to_string()
            } else {
                format!("{name}.png")
            }
        }
        None => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("screenshot-{ts}.png")
        }
    };

    let path = dir.join(&file_name);
    std::fs::write(&path, &bytes)?;

    Ok(path.display().to_string())
}

fn screenshot_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".aibrowsr").join("tmp"))
}

/// Minimal base64 decoder (RFC 4648). Avoids pulling in the `base64` crate.
fn base64_decode(input: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }

    let input = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' || b == b' ' {
            continue;
        }
        let val = lookup[b as usize];
        if val == 255 {
            return Err(format!("Invalid base64 character: {}", b as char).into());
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}
