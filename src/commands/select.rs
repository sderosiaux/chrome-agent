use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<crate::element::SelectOutcome, crate::BoxError> {
    Ok(crate::element::select_option(client, uid_map, uid, value).await?)
}
