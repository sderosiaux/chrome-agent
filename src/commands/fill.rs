use std::collections::HashMap;

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

/// Fill one field by uid. `secret` is the caller's own claim, on top of what the element
/// declares; it only ever ADDS redaction.
pub async fn run(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    uid: &str,
    value: &str,
    secret: bool,
) -> Result<(String, crate::element::FillOutcome), crate::BoxError> {
    let outcome = crate::element::fill_with(client, uid_map, uid, value, secret).await?;
    Ok((format!("Filled uid={uid} with {}", value.len()), outcome))
}

/// Fill several fields, returning each one's read-back rather than a count — a count cannot
/// show the mask that reformatted one of them.
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
    let names: Vec<String> = outcomes
        .iter()
        .map(|(uid, _)| format!("uid={uid}"))
        .collect();
    // The names, when there are any: "Filled 0 fields: " would end on a dangling colon.
    let message = if names.is_empty() {
        "Filled 0 fields".to_string()
    } else {
        format!("Filled {} fields: {}", names.len(), names.join(", "))
    };
    Ok((message, outcomes))
}
