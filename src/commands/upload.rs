use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(client: &CdpClient, uid_map: &HashMap<String, ElementRef>, uid: &str, files: &[String]) -> Result<String, crate::BoxError> {
    crate::element::set_file_input(client, uid_map, uid, files).await?;
    Ok(format!("Uploaded {} file(s) to uid={uid}", files.len()))
}
