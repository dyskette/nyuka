//! The per-`Store` resource table.
//!
//! The Aidoku ABI is resource-handle based: host functions return an `Rid`,
//! and `std::destroy` / `buffer_len` / `read_buffer` manage the lifetime and
//! transfer of host-owned objects. This table is what makes handles safe —
//! a handle from one invocation must never resolve in another.
