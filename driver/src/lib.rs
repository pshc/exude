#![feature(box_syntax, nonzero)]
#![recursion_limit = "1024"]
#![allow(unused_doc_comment)] // temp until error_chain updated

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

use comms::{Chan, Pipe, Wrapper};
use driver_abi::DriverCallbacks;
pub use errors::*;
use g::{DriverBox, DriverRef, DriverRefMut, Encoder, Res, gfx};
use g::gfx::IntoIndexBuffer;
use g::gfx::traits::{Factory, FactoryExt};
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
    DriverBox::new(ptr)
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
    goats: Option<u32>,
}

impl<P: Pipe> DriverState<P> {
    pub fn new(pipe: P) -> Self {
        DriverState { pipe, broken_comms: false, goats: None }
    }

    pub fn shutdown(self) -> P {
        self.pipe
    }

    fn handle_resp(&mut self, resp: api::DownResponse) {
        use api::DownResponse::*;
        match resp {
            Pong(n) => println!("Pong: {}", n),
            ProposeUpgrade(uri, info) => {
                let msg = proto::handshake::UpControl::Download(uri, info);
                self.pipe.send_on_chan(Chan::Control, &msg).expect("control write");
            }
            Goats(n) => self.goats = Some(n),
        }
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
    match RenderImpl::<Res, Wrapper>::new(state, factory, rtv)
        .and_then(|render| {
            render.update_goats(factory, None)?;
            Ok(render)
        })
    {
        Ok(render) => {
            let ptr = Box::into_raw(box render) as *mut ();
            g::GfxBox::new(ptr)
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

        let vertex_buffer = factory
            .create_buffer(
                3,
                gfx::buffer::Role::Vertex,
                gfx::memory::Usage::Upload,
                gfx::Bind::empty(),
            )
            .chain_err(|| "creating vertex buffer")?;
        let indices = [0u16, 1, 2].into_index_buffer(factory);
        let slice = gfx::Slice {
            start: 0,
            end: 3,
            base_vertex: 0,
            instances: None,
            buffer: indices
        };
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

    pub fn update_goats(&self, factory: &mut g::Factory, goats: Option<u32>) -> Result<()> {
        use std::f32::consts::PI;
        let off = goats.map(|n| ((n % 30) as f32 / 15.0 * PI).cos()).unwrap_or(0.0);
        let mut vbuf = factory
            .write_mapping(&self.data.vbuf)
            .chain_err(|| "writing vertex buffer")?;
        vbuf[0] = Vertex { pos: [-0.5, -0.5], color: [1.0, off, 0.0] };
        vbuf[1] = Vertex { pos: [0.5, -0.5], color: [0.0, 1.0, off] };
        vbuf[2] = Vertex { pos: [0.0, 0.5], color: [off, 0.0, 1.0] };
        Ok(())
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
    pub fn draw(&self, encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    pub fn update(&mut self, state: &mut DriverState<P>, factory: &mut g::Factory) {

        if !state.broken_comms {
            loop {
                match state.pipe.try_recv::<api::DownResponse>() {
                    Ok(None) => break,
                    Ok(Some(resp)) => state.handle_resp(resp),
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

        match self.update_goats(factory, state.goats) {
            Ok(()) => (),
            Err(e) => {
                use error_chain::ChainedError;
                panic!("{}", e.display());
            }
        }
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
