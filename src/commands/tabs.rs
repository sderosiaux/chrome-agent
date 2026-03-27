use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::GetTargetsResult;

pub async fn run(client: &CdpClient) -> Result<String, Box<dyn std::error::Error>> {
    let result: GetTargetsResult = client
        .call("Target.getTargets", json!({}))
        .await?;

    let pages: Vec<_> = result
        .target_infos
        .iter()
        .filter(|t| t.target_type == "page")
        .collect();

    if pages.is_empty() {
        return Ok("No open tabs.".into());
    }

    let mut output = String::new();
    output.push_str(&format!(
        "{:<36}  {:<50}  {}\n",
        "TARGET_ID", "URL", "TITLE"
    ));
    output.push_str(&"-".repeat(120));
    output.push('\n');

    for page in &pages {
        let url_display = if page.url.len() > 50 {
            format!("{}...", &page.url[..47])
        } else {
            page.url.clone()
        };
        output.push_str(&format!(
            "{:<36}  {:<50}  {}\n",
            page.target_id, url_display, page.title
        ));
    }

    Ok(output)
}
