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
pub extern crate winit;

pub mod macros;


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

        pub trait GlInterface {
            fn draw(&self, &GlCtx, &mut Encoder);
            fn setup(&self, &mut Factory, RenderTargetView) -> io::Result<Box<GlCtx>>;
            fn cleanup(&self, Box<GlCtx>);
        }

        pub type GlDrawFn = extern "C" fn(&GlCtx, &mut Encoder);
        pub type GlSetupFn = extern "C" fn(&mut Factory, RenderTargetView) -> io::Result<Box<GlCtx>>;
        pub type GlCleanupFn = extern "C" fn(Box<GlCtx>);

        pub trait GlCtx {
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
