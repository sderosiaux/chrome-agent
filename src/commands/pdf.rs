use std::path::PathBuf;

use serde_json::json;

use crate::cdp::client::CdpClient;

/// Options for `Page.printToPDF`.
pub struct PdfOpts<'a> {
    pub filename: Option<&'a str>,
    pub landscape: bool,
    pub background: bool,
}

#[derive(serde::Deserialize)]
struct PrintToPdfResult {
    data: String,
}

pub async fn run(client: &CdpClient, opts: &PdfOpts<'_>) -> Result<String, crate::BoxError> {
    // printToPDF requires the Page domain.
    client.enable("Page").await?;

    let result: PrintToPdfResult = client
        .call(
            "Page.printToPDF",
            json!({
                "landscape": opts.landscape,
                "printBackground": opts.background,
                // Return the bytes inline (base64) rather than a stream handle.
                "transferMode": "ReturnAsBase64",
            }),
        )
        .await?;

    let bytes = crate::base64::decode(&result.data)?;

    let dir = pdf_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(output_name(opts.filename));
    std::fs::write(&path, &bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(path.display().to_string())
}

/// Resolve the output file name, forcing a `.pdf` extension and stripping any path.
fn output_name(filename: Option<&str>) -> String {
    match filename {
        Some(name) => {
            let stem = std::path::Path::new(name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("page");
            if stem.to_ascii_lowercase().ends_with(".pdf") {
                stem.to_string()
            } else {
                format!("{stem}.pdf")
            }
        }
        None => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("page-{ts}.pdf")
        }
    }
}

fn pdf_dir() -> Result<PathBuf, crate::BoxError> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".chrome-agent").join("tmp"))
}

#[cfg(test)]
mod tests {
    use super::output_name;

    #[test]
    fn forces_pdf_extension() {
        assert_eq!(output_name(Some("report")), "report.pdf");
        assert_eq!(output_name(Some("report.pdf")), "report.pdf");
        assert_eq!(output_name(Some("REPORT.PDF")), "REPORT.PDF");
    }

    #[test]
    fn strips_path_traversal() {
        let n = output_name(Some("../../etc/passwd"));
        assert_eq!(n, "passwd.pdf");
        assert!(!n.contains('/'));
    }

    #[test]
    fn default_is_timestamped() {
        let n = output_name(None);
        assert!(n.starts_with("page-"));
        assert!(n.ends_with(".pdf"));
        // Guard against a "page-.pdf" bug: the stem between prefix and ext is all digits.
        let digits = n.trim_start_matches("page-").trim_end_matches(".pdf");
        assert!(!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    }
}
