use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(client: &CdpClient, uid_map: &HashMap<String, ElementRef>, from_uid: &str, to_uid: &str) -> Result<String, crate::BoxError> {
    crate::element::drag(client, uid_map, from_uid, to_uid).await?;
    Ok(format!("Dragged uid={from_uid} to uid={to_uid}"))
}
