pub extern crate gfx;
pub extern crate gfx_device_gl;
pub extern crate gfx_text;
pub extern crate gfx_window_glutin;
pub extern crate glutin;

use std::io;

pub mod macros;

pub type Res = gfx_device_gl::Resources;
pub type Command = gfx_device_gl::CommandBuffer;
pub type Factory = gfx_device_gl::Factory;
pub type ColorFormat = gfx::format::Rgba8;

pub type Encoder = gfx::Encoder<Res, Command>;
pub type RenderTargetView = gfx::handle::RenderTargetView<Res, ColorFormat>;

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
