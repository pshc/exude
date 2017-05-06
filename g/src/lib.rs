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
pub struct DriverHandle(pub *mut libc::c_void);
unsafe impl Send for DriverHandle {}

pub type ColorFormat = gfx::format::Rgba8;
pub type DepthFormat = gfx::format::Depth;

macro_rules! backend_items {
    ($backend:ident) => (
        use $backend;
        use std::io;
        use gfx;
        use super::{ColorFormat, DepthFormat};

        pub type Command = $backend::CommandBuffer;
        pub type Factory = $backend::Factory;
        pub type Res = $backend::Resources;

        pub type Encoder = gfx::Encoder<Res, Command>;
        pub type RenderTargetView = gfx::handle::RenderTargetView<Res, ColorFormat>;
        pub type DepthStencilView = gfx::handle::DepthStencilView<Res, DepthFormat>;

        pub trait GfxInterface {
            fn draw(&self, &GfxCtx, &mut Encoder);
            fn gfx_setup(&self, &mut Factory, RenderTargetView) -> io::Result<Box<GfxCtx>>;
            fn gfx_cleanup(&self, Box<GfxCtx>);
        }

        pub type GlDrawFn = extern "C" fn(&GfxCtx, &mut Encoder);
        pub type GlSetupFn = extern "C" fn(&mut Factory, RenderTargetView) -> io::Result<Box<GfxCtx>>;
        pub type GlCleanupFn = extern "C" fn(Box<GfxCtx>);

        pub trait GfxCtx {
            fn draw(&self, &mut Encoder);
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
