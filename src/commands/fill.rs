use std::collections::HashMap;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
) -> Result<(String, crate::element::FillOutcome), crate::BoxError> {
    let outcome = crate::element::fill(client, uid_map, uid, value).await?;
    Ok((format!("Filled uid={uid} with {}", value.len()), outcome))
}

/// Fill several fields, keeping what each one held afterwards.
///
/// The outcomes used to be dropped on the floor and the answer was a count. A count is
/// right about how many writes were attempted and silent about the mask that reformatted
/// one of them — the very thing `fill` returns `value` to expose.
pub async fn run_form(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    pairs: &[(&str, &str)],
) -> Result<(String, Vec<(String, crate::element::FillOutcome)>), crate::BoxError> {
    let mut outcomes = Vec::new();
    for (uid, value) in pairs {
        let outcome = crate::element::fill(client, uid_map, uid, value).await?;
        outcomes.push(((*uid).to_string(), outcome));
    }
    let names: Vec<String> = outcomes.iter().map(|(uid, _)| format!("uid={uid}")).collect();
    Ok((format!("Filled {} fields: {}", names.len(), names.join(", ")), outcomes))
}
