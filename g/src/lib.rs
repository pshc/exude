#![feature(nonzero)]

extern crate core;
pub extern crate gfx;
#[cfg(feature = "gl")]
pub extern crate gfx_device_gl;
#[cfg(feature = "metal")]
pub extern crate gfx_device_metal;
pub extern crate gfx_text;
#[cfg(feature = "gl")]
pub extern crate gfx_window_glutin;
#[cfg(feature = "metal")]
pub extern crate gfx_window_metal;
#[cfg(feature = "gl")]
pub extern crate glutin;
extern crate libc;
pub extern crate winit;

pub mod macros;

/// Opaque user pointer passed into non-gfx-related functions.
#[derive(Clone, Copy)]
pub struct DriverHandle(pub *const libc::c_void);
unsafe impl Send for DriverHandle {}

pub type ColorFormat = gfx::format::Rgba8;
pub type DepthFormat = gfx::format::Depth;

macro_rules! backend_items {
    ($backend:ident) => (
        use $backend;
        use core::nonzero::NonZero;
        use gfx;
        use super::{ColorFormat, DepthFormat, DriverHandle};

        pub type Command = $backend::CommandBuffer;
        pub type Factory = $backend::Factory;
        pub type Res = $backend::Resources;

        pub type Encoder = gfx::Encoder<Res, Command>;
        pub type RenderTargetView = gfx::handle::RenderTargetView<Res, ColorFormat>;
        pub type DepthStencilView = gfx::handle::DepthStencilView<Res, DepthFormat>;

        pub type GlDrawFn = extern "C" fn(GfxCtx, &mut Encoder);
        pub type GlUpdateFn = extern "C" fn(GfxCtx, DriverHandle);
        pub type GlSetupFn = extern "C" fn(DriverHandle, &mut Factory, RenderTargetView) -> Option<GfxCtx>;
        pub type GlCleanupFn = extern "C" fn(GfxCtx);

        /// Opaque user pointer passed into gfx-related functions.
        #[derive(Clone, Copy)]
        pub struct GfxCtx(pub NonZero<*mut ()>);
        unsafe impl Send for GfxCtx {}

        impl GfxCtx {
            pub unsafe fn new(ptr: *mut ()) -> Option<Self> {
                if ptr.is_null() {
                    None
                } else {
                    Some(GfxCtx(NonZero::new(ptr)))
                }
            }
        }
    )
}

#[cfg(feature = "gl")]
pub mod gl {
    backend_items!(gfx_device_gl);
}
#[cfg(all(feature = "gl", not(feature = "metal")))]
pub use gl::*;

#[cfg(feature = "metal")]
pub mod metal {
    backend_items!(gfx_device_metal);
}
#[cfg(all(not(feature = "gl"), feature = "metal"))]
pub use metal::*;

#[cfg(all(not(feature = "gl"), not(feature = "metal")))]
panic!("please enable at least one backend (e.g. cargo build --features=gl)");
