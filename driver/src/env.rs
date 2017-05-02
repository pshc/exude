//! Shared interface between the loader and driver.

use libc::c_void;

pub struct DriverCtx(pub *mut c_void);
unsafe impl Send for DriverCtx {}

/// For transmitting messages between driver and client core.
/// Uses C ABI in an attempt at interface stability.
#[repr(C)]
pub struct DriverEnv {
    /// Must be passed into all below functions.
    pub ctx: DriverCtx,

    /// Passed bytes will be copied to an internal buffer.
    /// On error, returns a negative value.
    pub send_fn: extern fn(*mut c_void, buf: *const u8, len: i32) -> i32,

    /// Attempt to receive a message, non-blocking.
    /// On message, writes the pointer to a new allocated buffer, and returns its length.
    /// If no messages are pending, returns zero.
    /// On error, returns a negative value.
    pub try_recv_fn: extern fn(*mut c_void, buf_out: *mut *mut u8) -> i32,

    /// Closes communication channels and frees memory.
    pub shutdown_fn: extern fn(*mut c_void),
}
