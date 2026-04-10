use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(client: &CdpClient, uid_map: &HashMap<String, ElementRef>, uid: &str, desired: bool) -> Result<String, crate::BoxError> {
    let msg = crate::element::set_checked(client, uid_map, uid, desired).await?;
    Ok(msg)
}
