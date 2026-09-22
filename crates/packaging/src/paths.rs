//! Path construction and sanitization.
//!
//! Every component derived from source data is attacker-influenced: titles,
//! chapter titles, and page filenames all originate in third-party WASM
//! (ADR-0004). This module is the security boundary for zip-slip and
//! traversal, and ADR-0007 asks for it to be fuzzed rather than only
//! unit-tested.
//!
//! Rules:
//!
//! - Strip path separators and control characters; reject `.` and `..`.
//! - Normalize Unicode to NFC, so a volume stays movable between ext4, APFS,
//!   and NTFS, which differ on both case sensitivity and normalization.
//! - Reject Windows reserved names.
//! - Collapse whitespace; cap each component at 255 **bytes** of UTF-8, not
//!   characters.
//! - **Canonicalize the result and assert it is under the library root.**
//!   Sanitizing inputs and checking the output are different controls; keep
//!   both.
