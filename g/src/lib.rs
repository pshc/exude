pub extern crate gfx;
pub extern crate gfx_device_gl;
pub extern crate gfx_text;
pub extern crate gfx_window_glutin;
pub extern crate glutin;

use std::io;

pub mod macros;

pub type Res = gfx_device_gl::Resources;
pub type Command = gfx_device_gl::CommandBuffer;

pub type ColorFormat = gfx::format::Rgba8;

pub type GlDrawFn = extern fn(data: &DrawGL, encoder: &mut gfx::Encoder<Res, Command>);
pub type GlSetupFn = extern fn(&mut gfx_device_gl::Factory,
                               Box<gfx::handle::RenderTargetView<Res, ColorFormat>>)
                               -> io::Result<Box<DrawGL>>;
pub type GlCleanupFn = extern fn(data: Box<DrawGL>);

pub trait DrawGL {
    fn draw(&self, &mut gfx::Encoder<Res, Command>);
}
