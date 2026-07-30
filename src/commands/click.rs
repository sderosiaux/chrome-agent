use std::collections::HashMap;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;
use crate::hit_test::{Dispatched, OnIntercept};

/// Click a uid, and return both the message and what the hit test saw.
///
/// The message comes from the outcome rather than a template: an aim point that never settled
/// means nothing was dispatched, and "Clicked uid=n9" would be false.
pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    on_intercept: OnIntercept,
) -> Result<(String, Dispatched), crate::BoxError> {
    let outcome = crate::element::click(client, uid_map, uid, on_intercept).await?;
    let target = format!("uid={uid}");
    let msg = outcome
        .refusal_message("click", &target)
        .unwrap_or_else(|| format!("Clicked {target}"));
    Ok((msg, outcome))
}
