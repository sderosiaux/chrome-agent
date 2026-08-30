use std::path::PathBuf;

use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::{CaptureScreenshotParams, CaptureScreenshotResult};
use crate::geometry::{self, Rect};

/// Image encoding for a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImgFormat {
    Png,
    Jpeg,
}

impl ImgFormat {
    /// Parse a user-supplied format string (case-insensitive; `jpg` aliases `jpeg`).
    pub fn parse(s: &str) -> Result<Self, crate::BoxError> {
        match s.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            other => {
                Err(format!("Unknown screenshot format {other:?}. Use \"png\" or \"jpeg\".").into())
            }
        }
    }

    /// CDP wire value for `Page.captureScreenshot.format`.
    const fn cdp(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    /// File extension (no dot).
    const fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

/// Options for a screenshot capture.
pub struct ScreenshotOpts<'a> {
    pub filename: Option<&'a str>,
    pub format: ImgFormat,
    /// JPEG quality 0-100. Ignored for PNG.
    pub quality: Option<u32>,
    /// Downscale so the captured width fits within this many CSS pixels.
    pub max_width: Option<u32>,
    /// Element clip rectangle (page CSS px). `None` captures the full page.
    pub clip: Option<Rect>,
}

pub async fn run(client: &CdpClient, opts: &ScreenshotOpts<'_>) -> Result<String, crate::BoxError> {
    // The clip rect and the base width the downscale is computed from. A full page with
    // `max_width` needs a whole-document clip, since `scale` only applies to a clip.
    let (clip, base_width) = match (opts.clip, opts.max_width) {
        (Some(rect), _) => (Some(rect), rect.width),
        (None, Some(_)) => {
            let rect = full_page_rect(client).await?;
            (Some(rect), rect.width)
        }
        (None, None) => (None, 0.0),
    };

    let scale = geometry::compute_scale(base_width, opts.max_width);

    let clip_value = clip.map(
        |r| json!({ "x": r.x, "y": r.y, "width": r.width, "height": r.height, "scale": scale }),
    );

    // CDP rejects `quality` on PNG.
    let quality = match opts.format {
        ImgFormat::Jpeg => opts.quality,
        ImgFormat::Png => None,
    };

    let result: CaptureScreenshotResult = client
        .call(
            "Page.captureScreenshot",
            CaptureScreenshotParams {
                format: Some(opts.format.cdp().into()),
                quality,
                clip: clip_value,
                from_surface: None,
                capture_beyond_viewport: clip.map(|_| true),
                optimize_for_speed: None,
            },
        )
        .await?;

    let bytes = crate::base64::decode(&result.data)?;

    let dir = screenshot_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(output_name(opts.filename, opts.format));
    std::fs::write(&path, &bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(path.display().to_string())
}

/// Resolve the output file name, forcing the extension to match the format.
fn output_name(filename: Option<&str>, format: ImgFormat) -> String {
    let ext = format.ext();
    match filename {
        Some(name) => {
            // Strip any path component, so a name cannot traverse.
            let stem = std::path::Path::new(name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("screenshot");
            if stem.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
                stem.to_string()
            } else {
                format!("{stem}.{ext}")
            }
        }
        None => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("screenshot-{ts}.{ext}")
        }
    }
}

/// Full-document rectangle, including content beyond the viewport.
async fn full_page_rect(client: &CdpClient) -> Result<Rect, crate::BoxError> {
    let eval: crate::cdp::types::EvaluateResult = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "(() => { const d = document.documentElement, b = document.body || d; \
                    return [Math.max(d.scrollWidth, b.scrollWidth), Math.max(d.scrollHeight, b.scrollHeight)]; })()",
                "returnByValue": true,
            }),
        )
        .await?;
    let dims = eval
        .result
        .value
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let width = dims
        .first()
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let height = dims
        .get(1)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    Ok(Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    })
}

fn screenshot_dir() -> Result<PathBuf, crate::BoxError> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_format_accepts_aliases() {
        assert_eq!(ImgFormat::parse("png").unwrap(), ImgFormat::Png);
        assert_eq!(ImgFormat::parse("PNG").unwrap(), ImgFormat::Png);
        assert_eq!(ImgFormat::parse("jpeg").unwrap(), ImgFormat::Jpeg);
        assert_eq!(ImgFormat::parse("JPG").unwrap(), ImgFormat::Jpeg);
    }

    #[test]
    fn parse_format_rejects_unknown() {
        assert!(ImgFormat::parse("webp").is_err());
    }

    #[test]
    fn extensions_match_format() {
        assert_eq!(ImgFormat::Png.ext(), "png");
        assert_eq!(ImgFormat::Jpeg.ext(), "jpg");
    }

    #[test]
    fn output_name_forces_extension() {
        assert_eq!(output_name(Some("shot"), ImgFormat::Png), "shot.png");
        assert_eq!(output_name(Some("shot"), ImgFormat::Jpeg), "shot.jpg");
        // A correct extension is preserved, a mismatched one is appended to.
        assert_eq!(output_name(Some("shot.jpg"), ImgFormat::Jpeg), "shot.jpg");
        assert_eq!(
            output_name(Some("shot.png"), ImgFormat::Jpeg),
            "shot.png.jpg"
        );
    }

    #[test]
    fn output_name_strips_path_traversal() {
        let n = output_name(Some("../../etc/passwd"), ImgFormat::Png);
        assert_eq!(n, "passwd.png");
        assert!(!n.contains('/'));
    }

    #[test]
    fn output_name_falls_back_when_no_file_component() {
        // ".." / "" have no file_name() component → stable "screenshot" stem.
        assert_eq!(output_name(Some(".."), ImgFormat::Png), "screenshot.png");
        assert_eq!(output_name(Some(""), ImgFormat::Jpeg), "screenshot.jpg");
    }

    #[test]
    fn output_name_default_is_timestamped() {
        let n = output_name(None, ImgFormat::Jpeg);
        assert!(n.starts_with("screenshot-"));
        assert!(n.ends_with(".jpg"));
    }
}
