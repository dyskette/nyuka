//! FFI result codes.
//!
//! An `FFIResult` is an `i32`: non-negative is success — a resource handle, a
//! length, or a value — and negative is an error.
//!
//! # Each module has its own code space
//!
//! This is the part that is easy to get wrong, and getting it wrong is silent.
//! `html::select_first` returning [`html::INVALID_QUERY`] instead of
//! [`html::NO_RESULT`] tells the guest its selector was malformed rather than
//! that the element is absent, and a source responds by abandoning the whole
//! entry instead of skipping one field. That produced a well-formed result
//! containing zero entries, with no error anywhere. See ADR-0004.

/// `AidokuError::error_code` in the guest. Returned from entry points and from
/// modules without a code space of their own.
pub mod aidoku {
    pub const MESSAGE: i32 = -1;
    pub const UNIMPLEMENTED: i32 = -2;
    pub const REQUEST: i32 = -3;
    pub const HTML: i32 = -4;
    pub const JS: i32 = -5;
    pub const CANVAS: i32 = -6;
    pub const UTF8: i32 = -7;
    pub const JSON_PARSE: i32 = -8;
    pub const DESERIALIZE: i32 = -9;
}

/// `HtmlError::from` in the guest. **Not** the same space as [`aidoku`].
pub mod html {
    /// The handle is not a node, or does not exist.
    pub const INVALID_DESCRIPTOR: i32 = -1;
    pub const INVALID_STRING: i32 = -2;
    pub const INVALID_HTML: i32 = -3;
    /// The selector itself could not be parsed.
    pub const INVALID_QUERY: i32 = -4;
    /// A legitimate miss: the selector was fine, nothing matched.
    pub const NO_RESULT: i32 = -5;
    pub const OTHER: i32 = -6;
}

/// `RequestError::from` in the guest.
pub mod net {
    pub const INVALID_DESCRIPTOR: i32 = -1;
    pub const INVALID_STRING: i32 = -2;
    pub const INVALID_URL: i32 = -3;
    pub const INVALID_METHOD: i32 = -4;
    pub const FAILED: i32 = -5;
    pub const NOT_SENT: i32 = -6;
}

/// Success for the "fill this buffer" imports.
///
/// `std::read_buffer` and `net::read_data` return **zero** on success, not the
/// number of bytes written: the guest does `if error != 0 { return None }`, so
/// returning a length makes every successful read look like a failure. This is
/// the other silent-failure trap in the ABI.
pub const READ_OK: i32 = 0;
