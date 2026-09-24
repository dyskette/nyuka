//! Host imports, mirroring `AidokuRunner` — which is a **Swift** package.
//! There is no Rust reference implementation, so parity is established by
//! reading Swift and testing against real sources.
//!
//! `abi/tier1-surface.txt` is the generated list of what tier 1 actually is at
//! the pinned commit, and a test asserts `bindings::PROVIDED` covers it.
//! Regenerate with `cargo xtask abi-surface` after re-pinning.
//!
//! Surface per module, from `aidoku-rs` `crates/lib/src/imports/`:
//!
//! - `std` — `destroy`, `buffer_len`, `read_buffer`, `current_date`,
//!   `utc_offset`, `parse_date`, `print`, `sleep`, `send_partial_result`.
//!   `send_partial_result` maps onto `job.progress` SSE events.
//! - `defaults` — `get`, `set`. Two functions, backed by `source_kv`.
//! - `net` — a stateful request builder: `init`, `set_url`, `set_header`,
//!   `set_body`, `set_timeout`, `send`, `send_all`, `data_len`, `read_data`,
//!   `get_image`, `get_header`, `get_status_code`, `get_url`, `html`,
//!   `set_rate_limit`. `send_all` issues the batch in bounded parallel.
//!   `get_image` answers a `canvas::ImageRef` and is therefore tier 3: it is
//!   refused at install by capability, not provided here.
//! - `html` — roughly 45 functions, and **mutating**: `set_attr`, `set_text`,
//!   `set_html`, `prepend`, `append`, `add_class`, `remove_class`, `remove`.
//!   A read-only parser cannot back this module.
//! - `js` — `context_*` is tier 2; `webview_*` is tier 3.
//! - `canvas` — tier 3.
//!
//! # Security
//!
//! `net` enforces an egress allow-list **after DNS resolution**, so a hostname
//! cannot resolve into the private range: reject loopback, RFC1918,
//! link-local, and cloud metadata addresses. ADR-0004 requires a DNS-rebinding
//! test for this, and fuzzing of the postcard decode path, which parses
//! attacker-influenced bytes into host structs.

pub mod bindings;
pub mod date;
pub mod egress;
pub mod html;
pub mod net;
