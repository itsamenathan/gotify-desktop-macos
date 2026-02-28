#[cfg(target_os = "macos")]
pub(crate) const LAUNCH_AGENT_LABEL: &str = "net.gotify.desktop";

pub(crate) const DEFAULT_CACHE_LIMIT: usize = 100;
pub(crate) const MAX_API_PAGE_LIMIT: usize = 200;
pub(crate) const MAX_CACHE_LIMIT: usize = 2000;

pub(crate) const STREAM_CONNECT_TIMEOUT_SECS: u64 = 10;
pub(crate) const STREAM_SYNC_INTERVAL_SECS: u64 = 5;
pub(crate) const STREAM_LIVENESS_CHECK_INTERVAL_SECS: u64 = 15;
pub(crate) const STREAM_LIVENESS_IDLE_SECS: u64 = 90;
pub(crate) const STREAM_LIVENESS_PING_GRACE_SECS: u64 = 30;

pub(crate) const PREVIEW_REQUEST_TIMEOUT_SECS: u64 = 6;
pub(crate) const PREVIEW_MAX_REDIRECTS: usize = 5;
pub(crate) const PREVIEW_MAX_HTML_BYTES: usize = 120_000;
pub(crate) const APP_ICON_MAX_BYTES: usize = 256_000;

pub(crate) const PAUSE_FOREVER_SENTINEL: u64 = 0;
pub(crate) const PAUSE_MODE_15M: &str = "15m";
pub(crate) const PAUSE_MODE_1H: &str = "1h";
pub(crate) const PAUSE_MODE_CUSTOM: &str = "custom";
pub(crate) const PAUSE_MODE_FOREVER: &str = "forever";
