//! Shared interface between the loader and driver.

use libc::c_void;

pub type VersionFn = extern "C" fn() -> u32;

/// The type of `io_spawn` which kicks off the driver's IO thread.
/// Must be passed `Box::into_raw(Box::new(DriverCallbacks {..}))`.
/// Returns an opaque thread::JoinHandle.
pub type IoSpawnFn = extern "C" fn(*mut DriverCallbacks) -> IoHandle;

/// The type of `io_join` which joins on the driver's IO thread.
/// Returns the boxed callback struct originally passed to `io_spawn`.
pub type IoJoinFn = extern "C" fn(IoHandle) -> *mut DriverCallbacks;

#[derive(Clone, Copy)]
pub struct DriverCtx(pub *mut c_void);
unsafe impl Send for DriverCtx {}

pub struct IoHandle(pub *mut c_void);
unsafe impl Send for IoHandle {}

/// For transmitting messages between driver and client core.
/// Uses C ABI in an attempt at interface stability.
#[repr(C)]
pub struct DriverCallbacks {
    /// Must be passed into all below functions.
    pub ctx: DriverCtx,

    /// Passed bytes will be copied to an internal buffer.
    /// On error, returns a negative value.
    pub send_fn: extern "C" fn(DriverCtx, buf: *const u8, len: i32) -> i32,

    /// Attempt to receive a message, non-blocking.
    /// On message, writes the pointer to a new allocated buffer, and returns its length.
    /// If no messages are pending, returns zero.
    /// On error, returns a negative value.
    pub try_recv_fn: extern "C" fn(DriverCtx, buf_out: *mut *mut u8) -> i32,
}
