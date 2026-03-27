use crate::cdp::client::CdpClient;
use crate::snapshot::Snapshot;

pub async fn run(
    client: &CdpClient,
    verbose: bool,
) -> Result<Snapshot, Box<dyn std::error::Error>> {
    let snapshot = crate::snapshot::take_snapshot(client, verbose).await?;
    Ok(snapshot)
}
