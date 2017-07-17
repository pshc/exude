#![feature(box_syntax, nonzero)]
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate g;
extern crate proto;

pub mod comms;
pub mod errors;
mod driver_abi;

use std::io::{self, Write};
use std::marker::PhantomData;

use comms::{Pipe, Wrapper};
use driver_abi::DriverCallbacks;
pub use errors::*;
use g::{DriverBox, DriverRef, DriverRefMut, Encoder, Res, gfx};
use g::gfx::traits::FactoryExt;
use proto::api;

#[no_mangle]
pub extern "C" fn version() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn setup(cbs: *mut DriverCallbacks) -> Option<DriverBox> {
    let pipe = Wrapper::new(cbs);
    let state = DriverState::new(pipe);
    let ptr = Box::into_raw(box state) as *mut ();
    unsafe { DriverBox::new(ptr) }
}

#[no_mangle]
pub extern "C" fn teardown(handle: DriverBox) -> *mut DriverCallbacks {
    let boxed = unsafe { Box::from_raw(handle.consume() as *mut DriverState<Wrapper>) };
    let wrapper: Wrapper = boxed.shutdown();
    wrapper.consume()
}

/// This is what we stash inside DriverBox.
pub struct DriverState<P> {
    pipe: P,
    broken_comms: bool,
}

impl<P> DriverState<P> {
    pub fn new(pipe: P) -> Self {
        DriverState { pipe, broken_comms: false }
    }

    pub fn shutdown(self) -> P {
        self.pipe
    }
}

mod simple {
    use g;
    use gfx;

    gfx_defines! {
        vertex Vertex {
            pos: [f32; 2] = "a_Pos",
            color: [f32; 3] = "a_Color",
        }

        pipeline pipe {
            vbuf: gfx::VertexBuffer<Vertex> = (),
            out: gfx::RenderTarget<g::ColorFormat> = "Target0",
        }
    }
}
use simple::{Vertex, pipe};

const TRIANGLE: [Vertex; 3] = [
    Vertex { pos: [-0.5, -0.5], color: [1.0, 0.0, 0.0] },
    Vertex { pos: [0.5, -0.5], color: [0.0, 1.0, 0.0] },
    Vertex { pos: [0.0, 0.5], color: [0.0, 0.0, 1.0] },
];

/// Convert a Newtype(NonZero<*T>) to &T.
macro_rules! cast_ptr {
    ($ptr:ident as &mut $t:ty) => {(
        &mut *($ptr.0.get() as *mut $t)
    )};
    ($ptr:ident as & $t:ty) => {(
        & *($ptr.0.get() as *const $t)
    )};
}

#[no_mangle]
pub extern "C" fn gl_setup(
    state_ref: DriverRef,
    factory: &mut g::Factory,
    rtv: g::RenderTargetView,
) -> Option<g::GfxBox> {

    let state = unsafe { cast_ptr!(state_ref as &DriverState<Wrapper>) };
    match RenderImpl::<Res, Wrapper>::new(state, factory, rtv) {
        Ok(render) => {
            let ptr = Box::into_raw(box render) as *mut ();
            unsafe { g::GfxBox::new(ptr) }
        }
        Err(e) => {
            let _ = writeln!(io::stderr(), "Driver setup: {}", e);
            None
        }
    }
}

pub struct RenderImpl<R: gfx::Resources, P> {
    slice: gfx::Slice<R>,
    pso: gfx::PipelineState<R, pipe::Meta>,
    data: pipe::Data<R>,
    _phantom: PhantomData<P>,
}

impl<P: Pipe> RenderImpl<Res, P> {
    pub fn new(
        _: &DriverState<P>,
        factory: &mut g::Factory,
        rtv: g::RenderTargetView,
    ) -> Result<Self> {
        let pso = factory
            .create_pipeline_simple(
                include_bytes!("shader/triangle_150.glslv"),
                include_bytes!("shader/triangle_150.glslf"),
                pipe::new(),
            )
            .chain_err(|| "couldn't set up gfx pipeline")?;

        let (vertex_buffer, slice) = factory.create_vertex_buffer_with_slice(&TRIANGLE, ());
        let data = pipe::Data { vbuf: vertex_buffer, out: rtv };

        Ok(
            RenderImpl {
                slice,
                pso,
                data,
                _phantom: PhantomData,
            }
        )
    }
}

#[no_mangle]
pub extern "C" fn gl_update(ctx: g::GfxRefMut, state_ref: DriverRefMut, factory: &mut g::Factory) {
    let render = unsafe { cast_ptr!(ctx as &mut RenderImpl<Res, Wrapper>) };
    let state = unsafe { cast_ptr!(state_ref as &mut DriverState<Wrapper>) };
    render.update(state, factory);
}

#[no_mangle]
pub extern "C" fn gl_draw(ctx: g::GfxRef, encoder: &mut Encoder) {
    let render = unsafe { cast_ptr!(ctx as &RenderImpl<Res, Wrapper>) };
    render.draw(encoder);
}

impl<P: Pipe> RenderImpl<Res, P> {
    pub fn draw(&self, mut encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    pub fn update(&mut self, state: &mut DriverState<P>, factory: &mut g::Factory) {

        if !state.broken_comms {
            loop {
                match state.pipe.try_recv::<api::DownResponse>() {
                    Ok(None) => break,
                    Ok(Some(msg)) => println!("=== {:?} ===", msg),
                    Err(Error(ErrorKind::BrokenComms, _)) => {
                        println!("=== COMMS BROKEN ===");
                        state.broken_comms = true;
                        break;
                    }
                    Err(e) => {
                        use error_chain::ChainedError;
                        panic!("{}", e.display());
                    }
                }
            }
        }

        // update gpu state here...
        let _ = factory;
    }
}

#[no_mangle]
pub extern "C" fn gl_cleanup(ctx: g::GfxBox) {
    let render = unsafe { Box::from_raw(ctx.consume() as *mut RenderImpl<Res, Wrapper>) };
    drop(render);
    println!("cleaned up GL");
}

#[allow(dead_code)]
fn check_gl_types() {
    let _: driver_abi::VersionFn = version;
    let _: driver_abi::SetupFn = setup;
    let _: driver_abi::TeardownFn = teardown;
    let _: g::GlSetupFn = gl_setup;
    let _: g::GlDrawFn = gl_draw;
    let _: g::GlUpdateFn = gl_update;
    let _: g::GlCleanupFn = gl_cleanup;
}
