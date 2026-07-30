use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::hit_test::OnIntercept;

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    desired: bool,
    on_intercept: OnIntercept,
) -> Result<crate::element::CheckOutcome, crate::BoxError> {
    Ok(crate::element::set_checked(client, uid_map, uid, desired, on_intercept).await?)
}
