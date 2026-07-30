use std::collections::HashMap;
use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::hit_test::{Dispatched, OnIntercept};

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    on_intercept: OnIntercept,
) -> Result<(String, Dispatched), crate::BoxError> {
    let outcome = crate::element::dblclick(client, uid_map, uid, on_intercept).await?;
    let target = format!("uid={uid}");
    let msg = outcome
        .refusal_message("double-click", &target)
        .unwrap_or_else(|| format!("Double-clicked {target}"));
    Ok((msg, outcome))
}
