//! What every dispatcher needs to act on one page, in one struct.
//!
//! The same eight values — two clients, the store, the three names that locate a page in it,
//! and the two global flags — used to be threaded by hand through ~30 signatures, so adding a
//! ninth meant editing every function in between. The per-command `cmd`/`msg` stays a separate
//! parameter: it is data, not context.

use crate::cdp::client::CdpClient;
use crate::run_helpers::ReportPolicy;
use crate::session::SessionStore;

pub struct PageCtx<'a> {
    pub client: &'a CdpClient,
    /// Browser-level connection: only `Target.*` works on it (`tabs`).
    pub browser_client: &'a CdpClient,
    /// `&mut` because half the dispatchers write the uid map and the snapshot back. A
    /// read-only dispatcher takes `&PageCtx` and reaches `&SessionStore` through it, so the
    /// read/write split still shows in the signature.
    pub store: &'a mut SessionStore,
    pub browser: &'a str,
    pub page: &'a str,
    pub target_id: &'a str,
    /// `--timeout`, seconds.
    pub timeout: u64,
    /// Global `--max-depth`; a command's own flag takes precedence.
    pub max_depth: Option<usize>,
    pub report: ReportPolicy,
}

impl PageCtx<'_> {
    /// This page's stored uid map, cloned. The one lookup every uid-targeted verb makes.
    #[must_use]
    pub fn uid_map(&self) -> std::collections::HashMap<String, crate::element_ref::ElementRef> {
        crate::run_helpers::get_uid_map(self.store, self.browser, self.page)
    }

    /// Take this reading as the page's baseline ([`crate::session::PageSession::store_snapshot`]).
    /// A no-op when the store holds no entry for this browser, which is what every hand-written
    /// copy of these lines did.
    pub fn store_snapshot(&mut self, snapshot: crate::snapshot::Snapshot) {
        if let Some(browser) = self.store.browsers.get_mut(self.browser) {
            crate::session::ensure_page(browser, self.page, self.target_id)
                .store_snapshot(snapshot);
        }
    }

    /// The uid map alone, cleared. A document that was replaced takes its uids with it:
    /// `backendNodeId` counters overlap between documents, so a stale uid resolves to an
    /// unrelated node on the new page. `last_snapshot` deliberately survives, so `diff` can
    /// answer `document_changed` instead of erroring.
    pub fn clear_uid_map(&mut self) {
        if let Some(browser) = self.store.browsers.get_mut(self.browser) {
            crate::session::ensure_page(browser, self.page, self.target_id)
                .uid_map
                .clear();
        }
    }

    /// This page's stored state, when it has one.
    #[must_use]
    pub fn page_state(&self) -> Option<&crate::session::PageSession> {
        self.store
            .browsers
            .get(self.browser)
            .and_then(|b| b.pages.get(self.page))
    }
}
