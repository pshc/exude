//! Shared interface between the loader and driver.

use g::DriverBox;

pub type VersionFn = extern "C" fn() -> u32;

/// The type of `setup` which is the first point of entry for the driver.
/// Must be passed `Box::into_raw(Box::new(DriverCallbacks {..}))`.
/// Returns an opaque pointer passed into subsequent driver calls.
pub type SetupFn = extern "C" fn(*mut DriverCallbacks) -> Option<DriverBox>;

/// The type of `teardown` which is the final call to the driver.
/// Returns the boxed callback struct originally passed to `setup`.
pub type TeardownFn = extern "C" fn(DriverBox) -> *mut DriverCallbacks;

/// Opaque context pointer provided by the loader.
#[derive(Clone, Copy)]
pub struct CallbackCtx(pub *mut ());

/// For transmitting messages between driver and client core.
/// Uses C ABI in an attempt at interface stability.
#[repr(C)]
pub struct DriverCallbacks {
    /// Must be passed into all below functions.
    pub ctx: CallbackCtx,

    /// Allocates a packet for use by `send_fn`.
    pub alloc_fn: extern "C" fn(CallbackCtx, len: i32) -> *mut u8,

    /// Takes ownership of the passed packet.
    /// On error, returns a negative value.
    pub send_fn: extern "C" fn(CallbackCtx, packet: *mut u8, len: i32) -> i32,
    pub control_write_fn: extern "C" fn(CallbackCtx, packet: *mut u8, len: i32) -> i32,

    /// Attempt to receive a message, non-blocking.
    /// On message, writes the pointer to a new allocated packet, and returns its length.
    /// If no messages are pending, returns zero.
    /// On error, returns a negative value.
    pub try_recv_fn: extern "C" fn(CallbackCtx, packet_out: *mut *mut u8) -> i32,

    /// Frees packets obtained from `try_recv_fn` or `alloc_fn`.
    pub free_fn: extern "C" fn(CallbackCtx, packet: *mut u8, len: i32),
}
