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

/// Opaque user pointer passed into all driver functions.
#[derive(Clone, Copy)]
pub struct DriverHandle(pub *const libc::c_void);
unsafe impl Send for DriverHandle {}

pub type ColorFormat = gfx::format::Rgba8;
pub type DepthFormat = gfx::format::Depth;

macro_rules! backend_items {
    ($backend:ident) => (
        use $backend;
        // XXX stop using this; io::Error not DLL-safe!
        use std::io;
        use gfx;
        use super::{ColorFormat, DepthFormat, DriverHandle};

        pub type Command = $backend::CommandBuffer;
        pub type Factory = $backend::Factory;
        pub type Res = $backend::Resources;

        pub type Encoder = gfx::Encoder<Res, Command>;
        pub type RenderTargetView = gfx::handle::RenderTargetView<Res, ColorFormat>;
        pub type DepthStencilView = gfx::handle::DepthStencilView<Res, DepthFormat>;

        pub trait GfxInterface {
            fn draw(&self, &GfxCtx, &mut Encoder);
            fn update(&self, &mut GfxCtx);
            fn gfx_setup(&self, &mut Factory, RenderTargetView) -> io::Result<Box<GfxCtx>>;
            fn gfx_cleanup(&self, Box<GfxCtx>);
        }

        pub type GlDrawFn = extern "C" fn(&GfxCtx, &mut Encoder);
        pub type GlUpdateFn = extern "C" fn(&mut GfxCtx, DriverHandle);
        pub type GlSetupFn = extern "C" fn(DriverHandle, &mut Factory, RenderTargetView) -> io::Result<Box<GfxCtx>>;
        pub type GlCleanupFn = extern "C" fn(Box<GfxCtx>);

        pub trait GfxCtx {
            fn draw(&self, &mut Encoder);
            fn update(&mut self, DriverHandle);
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
