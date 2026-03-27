use std::collections::HashMap;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    crate::element::click(client, uid_map, uid).await?;
    Ok(format!("Clicked uid={uid}"))
}
