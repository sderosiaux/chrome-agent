use std::collections::HashMap;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    crate::element::fill(client, uid_map, uid, value).await?;
    Ok(format!("Filled uid={uid} with {}", value.len(), ))
}

pub async fn run_form(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    pairs: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut filled = Vec::new();
    for (uid, value) in pairs {
        crate::element::fill(client, uid_map, uid, value).await?;
        filled.push(format!("uid={uid}"));
    }
    Ok(format!("Filled {} fields: {}", filled.len(), filled.join(", ")))
}
