use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;

use crate::cdp::client::CdpClient;
use crate::cdp::types::GetTargetsResult;
use crate::session::SessionStore;

/// Structured tab info for JSON output.
#[derive(Debug, Serialize)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

/// Build a `target_id` → `page_name` lookup from the session store.
fn page_labels(store: &SessionStore) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for browser in store.browsers.values() {
        for (name, page) in &browser.pages {
            map.insert(page.target_id.clone(), name.clone());
        }
    }
    map
}

/// Return structured tab data.
pub async fn run_structured(
    client: &CdpClient,
    store: &SessionStore,
) -> Result<Vec<TabInfo>, crate::BoxError> {
    let result: GetTargetsResult = client.call("Target.getTargets", json!({})).await?;

    let labels = page_labels(store);

    let tabs = result
        .target_infos
        .into_iter()
        .filter(|t| t.target_type == "page")
        .map(|t| {
            let page = labels.get(&t.target_id).cloned();
            TabInfo {
                id: t.target_id,
                url: t.url,
                title: t.title,
                page,
            }
        })
        .collect();

    Ok(tabs)
}

/// Return formatted text output.
pub async fn run(client: &CdpClient, store: &SessionStore) -> Result<String, crate::BoxError> {
    let tabs = run_structured(client, store).await?;

    if tabs.is_empty() {
        return Ok("No open tabs.".into());
    }

    let mut output = String::new();
    output.push_str(&format!(
        "{:<10}  {:<36}  {:<50}  {}\n",
        "PAGE", "TARGET_ID", "URL", "TITLE"
    ));
    output.push_str(&"-".repeat(130));
    output.push('\n');

    for tab in &tabs {
        let page_label = tab.page.as_deref().unwrap_or("-");
        let url_display = crate::truncate::truncate_str(&tab.url, 47, "...");
        output.push_str(&format!(
            "{:<10}  {:<36}  {:<50}  {}\n",
            page_label, tab.id, url_display, tab.title
        ));
    }

    Ok(output)
}
