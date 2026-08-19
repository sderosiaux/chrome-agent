//! Three-way merge for processes that share one named browser.
//!
//! `loaded` is this process's baseline, `current` is its proposed state, and `on_disk` is the
//! latest state written by every process. Within one browser and target lifetime, fields changed
//! locally stay local; unchanged fields inherit their latest value from `on_disk`.

use std::collections::HashSet;

use serde::Serialize;

use crate::session::{BrowserSession, PageSession};

/// Return whether two entries describe the same browser process lifetime.
#[must_use]
pub fn same_browser_lifetime(left: &BrowserSession, right: &BrowserSession) -> bool {
    left.ws_endpoint == right.ws_endpoint && left.pid == right.pid
}

/// Merge three versions of one browser entry without carrying state across process lifetimes.
pub fn merge_browser_entry(
    loaded: &BrowserSession,
    current: &BrowserSession,
    on_disk: &BrowserSession,
) -> BrowserSession {
    if !same_browser_lifetime(loaded, current) {
        // This process deliberately replaced the browser it loaded; its new lifetime wins.
        return current.clone();
    }
    if !same_browser_lifetime(loaded, on_disk) {
        // Another process replaced the browser first. An older process must not resurrect the
        // endpoint, pid, pages, or emulation state belonging to the previous Chrome process.
        return on_disk.clone();
    }

    let mut result = current.clone();
    // Import each browser field only when this process left it untouched. If both processes
    // changed one field, the explicit local value remains the conflict winner.
    if loaded.headless == current.headless {
        result.headless = on_disk.headless;
    }
    if loaded.proxy_server == current.proxy_server {
        result.proxy_server.clone_from(&on_disk.proxy_server);
    }
    if loaded.daemon_pid == current.daemon_pid {
        result.daemon_pid = on_disk.daemon_pid;
    }
    if loaded.closing == current.closing {
        result.closing = on_disk.closing;
    }
    let loaded_clients: HashSet<u32> = loaded.client_pids.iter().copied().collect();
    let current_clients: HashSet<u32> = current.client_pids.iter().copied().collect();
    let mut merged_clients: HashSet<u32> = on_disk.client_pids.iter().copied().collect();
    for removed in loaded_clients.difference(&current_clients) {
        merged_clients.remove(removed);
    }
    merged_clients.extend(current_clients.difference(&loaded_clients));
    result.client_pids = merged_clients.into_iter().collect();
    result.client_pids.sort_unstable();

    let all_page_names: HashSet<&String> = loaded
        .pages
        .keys()
        .chain(current.pages.keys())
        .chain(on_disk.pages.keys())
        .collect();
    for page_name in all_page_names {
        match (
            loaded.pages.get(page_name),
            current.pages.get(page_name),
            on_disk.pages.get(page_name),
        ) {
            (Some(loaded), Some(current), Some(on_disk)) => {
                result.pages.insert(
                    page_name.clone(),
                    merge_page_entry(loaded, current, on_disk),
                );
            }
            (None, None, Some(on_disk)) => {
                // Preserve a page another process added after this process loaded its baseline.
                result.pages.insert(page_name.clone(), on_disk.clone());
            }
            (Some(loaded), Some(current), None) if serialized_equal(loaded, current) => {
                // Honor a concurrent deletion only when this process left that page untouched.
                result.pages.remove(page_name);
            }
            // Every remaining shape contains a local addition, update, or deletion. `result`
            // already carries that explicit local decision.
            _ => {}
        }
    }
    result
}

/// Merge independent state attached to one named page.
///
/// A target replacement invalidates target-bound caches (`uid_map` and snapshots). Requested
/// emulation is different: it belongs to the named page and is intentionally carried to whichever
/// target now implements that page.
fn merge_page_entry(
    loaded: &PageSession,
    current: &PageSession,
    on_disk: &PageSession,
) -> PageSession {
    if loaded.target_id != current.target_id {
        let mut result = current.clone();
        if loaded.device_emulation == current.device_emulation {
            result
                .device_emulation
                .clone_from(&on_disk.device_emulation);
        }
        return result;
    }
    if loaded.target_id != on_disk.target_id {
        let mut result = on_disk.clone();
        if loaded.device_emulation != current.device_emulation {
            result
                .device_emulation
                .clone_from(&current.device_emulation);
        }
        return result;
    }

    let mut result = current.clone();
    // The uid map, rendered snapshot, and document identity are one cache. Splitting concurrent
    // versions could make snapshot text resolve through uids from a different page read.
    let loaded_cache = (
        &loaded.uid_map,
        &loaded.last_snapshot,
        &loaded.last_snapshot_frame,
        &loaded.last_snapshot_loader,
    );
    let current_cache = (
        &current.uid_map,
        &current.last_snapshot,
        &current.last_snapshot_frame,
        &current.last_snapshot_loader,
    );
    if serialized_equal(&loaded_cache, &current_cache) {
        result.uid_map.clone_from(&on_disk.uid_map);
        result.last_snapshot.clone_from(&on_disk.last_snapshot);
        result
            .last_snapshot_frame
            .clone_from(&on_disk.last_snapshot_frame);
        result
            .last_snapshot_loader
            .clone_from(&on_disk.last_snapshot_loader);
    }
    if loaded.device_emulation == current.device_emulation {
        result
            .device_emulation
            .clone_from(&on_disk.device_emulation);
    }
    result
}

/// Compare fields exactly as session persistence represents them. A serialization failure is not
/// evidence that two values are unchanged, so it conservatively returns false.
fn serialized_equal<T: Serialize>(left: &T, right: &T) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn browser(endpoint: &str) -> BrowserSession {
        BrowserSession {
            ws_endpoint: endpoint.into(),
            pid: Some(1),
            headless: true,
            proxy_server: None,
            daemon_pid: None,
            closing: false,
            client_pids: Vec::new(),
            pages: HashMap::new(),
        }
    }

    fn page(target_id: &str) -> PageSession {
        PageSession {
            target_id: target_id.into(),
            uid_map: HashMap::new(),
            last_snapshot: None,
            last_snapshot_frame: None,
            last_snapshot_loader: None,
            device_emulation: None,
        }
    }

    fn device_config() -> crate::emulation::DeviceEmulation {
        crate::emulation::DeviceEmulation::new(None, 390, 844, 3.0, true, true, None).unwrap()
    }

    fn set_snapshot_cache(
        page: &mut PageSession,
        uid: &str,
        backend_node_id: i64,
        snapshot: &str,
        frame: &str,
        loader: &str,
    ) {
        page.uid_map.clear();
        page.uid_map.insert(
            uid.into(),
            crate::element_ref::ElementRef::backend_node(backend_node_id),
        );
        page.last_snapshot = Some(snapshot.into());
        page.last_snapshot_frame = Some(frame.into());
        page.last_snapshot_loader = Some(loader.into());
    }

    #[test]
    fn unregistering_one_client_preserves_a_concurrent_registration() {
        let mut loaded = browser("ws://shared");
        loaded.client_pids.push(10);
        let mut current = loaded.clone();
        current.client_pids.clear();
        let mut on_disk = loaded.clone();
        on_disk.client_pids.push(20);

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        assert_eq!(merged.client_pids, vec![20]);
    }

    #[test]
    fn preserves_a_concurrently_added_sibling_page() {
        let mut loaded = browser("ws://shared");
        loaded.pages.insert("mobile".into(), page("mobile-target"));

        let mut current = loaded.clone();
        current.pages.get_mut("mobile").unwrap().last_snapshot = Some("updated by pipe".into());

        let mut on_disk = loaded.clone();
        on_disk
            .pages
            .insert("desktop".into(), page("desktop-target"));

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        assert_eq!(merged.pages.len(), 2);
        assert_eq!(
            merged.pages["mobile"].last_snapshot.as_deref(),
            Some("updated by pipe")
        );
        assert_eq!(merged.pages["desktop"].target_id, "desktop-target");
    }

    #[test]
    fn combines_a_concurrent_snapshot_cache_with_an_emulation_update() {
        let mut loaded = browser("ws://shared");
        loaded.pages.insert("mobile".into(), page("mobile-target"));

        let mut current = loaded.clone();
        current.pages.get_mut("mobile").unwrap().device_emulation = Some(device_config());

        let mut on_disk = loaded.clone();
        set_snapshot_cache(
            on_disk.pages.get_mut("mobile").unwrap(),
            "n1",
            1,
            "disk snapshot",
            "disk-frame",
            "disk-loader",
        );

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        let page = &merged.pages["mobile"];
        assert!(page.device_emulation.is_some());
        assert!(page.uid_map.contains_key("n1"));
        assert_eq!(page.last_snapshot.as_deref(), Some("disk snapshot"));
        assert_eq!(page.last_snapshot_frame.as_deref(), Some("disk-frame"));
        assert_eq!(page.last_snapshot_loader.as_deref(), Some("disk-loader"));
    }

    #[test]
    fn keeps_one_snapshot_cache_when_both_processes_refresh_it() {
        let mut loaded = browser("ws://shared");
        loaded.pages.insert("mobile".into(), page("mobile-target"));

        let mut current = loaded.clone();
        set_snapshot_cache(
            current.pages.get_mut("mobile").unwrap(),
            "n-local",
            1,
            "local snapshot",
            "local-frame",
            "local-loader",
        );

        let mut on_disk = loaded.clone();
        set_snapshot_cache(
            on_disk.pages.get_mut("mobile").unwrap(),
            "n-disk",
            2,
            "disk snapshot",
            "disk-frame",
            "disk-loader",
        );

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        let page = &merged.pages["mobile"];
        assert!(page.uid_map.contains_key("n-local"));
        assert!(!page.uid_map.contains_key("n-disk"));
        assert_eq!(page.last_snapshot.as_deref(), Some("local snapshot"));
        assert_eq!(page.last_snapshot_frame.as_deref(), Some("local-frame"));
        assert_eq!(page.last_snapshot_loader.as_deref(), Some("local-loader"));
    }

    #[test]
    fn concurrent_target_replacement_keeps_local_emulation_update() {
        let mut loaded = browser("ws://shared");
        loaded.pages.insert("mobile".into(), page("old-target"));

        let mut current = loaded.clone();
        current.pages.get_mut("mobile").unwrap().device_emulation = Some(device_config());

        let mut on_disk = loaded.clone();
        on_disk.pages.get_mut("mobile").unwrap().target_id = "new-target".into();

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        assert_eq!(merged.pages["mobile"].target_id, "new-target");
        assert!(merged.pages["mobile"].device_emulation.is_some());
    }

    #[test]
    fn local_target_replacement_keeps_concurrent_emulation_update() {
        let mut loaded = browser("ws://shared");
        loaded.pages.insert("mobile".into(), page("old-target"));

        let mut current = loaded.clone();
        current.pages.get_mut("mobile").unwrap().target_id = "new-target".into();

        let mut on_disk = loaded.clone();
        on_disk.pages.get_mut("mobile").unwrap().device_emulation = Some(device_config());

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        assert_eq!(merged.pages["mobile"].target_id, "new-target");
        assert!(merged.pages["mobile"].device_emulation.is_some());
    }

    #[test]
    fn a_concurrent_browser_replacement_wins_over_the_old_process() {
        let loaded = browser("ws://old");
        let current = loaded.clone();
        let mut on_disk = browser("ws://new");
        on_disk.pid = Some(2);

        let merged = merge_browser_entry(&loaded, &current, &on_disk);
        assert_eq!(merged.ws_endpoint, "ws://new");
        assert_eq!(merged.pid, Some(2));
    }
}
