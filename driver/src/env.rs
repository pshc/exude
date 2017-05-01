//! Shared interface between the loader and driver.

use std::io;

use g::gfx;
use g::gfx_device_gl;
use libc::c_void;

#[derive(Debug, Deserialize, Serialize)]
pub enum UpRequest {
    Ping(u32),
    Bye,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum DownResponse {
    Pong(u32),
}

// future work: macro for generating multiple backends (vulkan, ...)
pub type Res = gfx_device_gl::Resources;
pub type Command = gfx_device_gl::CommandBuffer;

pub type ColorFormat = gfx::format::Rgba8;

pub type GlDrawFn = extern fn(data: &DrawGL, encoder: &mut gfx::Encoder<Res, Command>);
pub type GlSetupFn = extern fn(&mut gfx_device_gl::Factory,
                               gfx::handle::RenderTargetView<gfx_device_gl::Resources, ColorFormat>)
                               -> io::Result<Box<DrawGL>>;
pub type GlCleanupFn = extern fn(data: Box<DrawGL>);

pub trait DrawGL {
    fn draw(&self, &mut gfx::Encoder<Res, Command>);
}

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
