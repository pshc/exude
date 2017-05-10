#![feature(box_syntax)]

#[macro_use]
extern crate g;
extern crate libc;
extern crate proto;

pub mod comms;
mod driver_abi;

use std::io::{self, ErrorKind, Write};
use std::marker::PhantomData;

use comms::{Pipe, Wrapper};
use driver_abi::DriverCallbacks;
use g::{DriverBox, DriverRef, Encoder, Res, gfx};
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
    match state {
        Ok(state) => {
            let ptr = Box::into_raw(box state) as *mut ();
            unsafe { DriverBox::new(ptr) }
        }
        Err(e) => {
            let _ = writeln!(io::stderr(), "Driver setup: {}", e);
            None
        }
    }
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
}

impl<P> DriverState<P> {
    pub fn new(pipe: P) -> io::Result<Self> {
        let state = DriverState { pipe };
        Ok(state)
    }

    pub fn shutdown(self) -> P {
        self.pipe
    }
}

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

const TRIANGLE: [Vertex; 3] = [
    Vertex { pos: [-0.5, -0.5], color: [1.0, 0.0, 0.0] },
    Vertex { pos: [0.5, -0.5], color: [0.0, 1.0, 0.0] },
    Vertex { pos: [0.0, 0.5], color: [0.0, 0.0, 1.0] },
];

#[no_mangle]
pub extern "C" fn gl_setup(
    state_ref: DriverRef,
    factory: &mut g::Factory,
    rtv: g::RenderTargetView,
) -> Option<g::GfxBox> {

    let state: *const DriverState<Wrapper> = *state_ref.0 as *const DriverState<Wrapper>;
    let state: &DriverState<Wrapper> = unsafe { &*state };
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
    ) -> io::Result<Self> {
        let pso = factory
            .create_pipeline_simple(
                include_bytes!("shader/triangle_150.glslv"),
                include_bytes!("shader/triangle_150.glslf"),
                pipe::new(),
            )
            .map_err(|e| io::Error::new(ErrorKind::Other, e))?;

        let (vertex_buffer, slice) = factory.create_vertex_buffer_with_slice(&TRIANGLE, ());
        let data = pipe::Data { vbuf: vertex_buffer, out: rtv };

        Ok(RenderImpl { slice, pso, data, _phantom: PhantomData })
    }
}

#[no_mangle]
pub extern "C" fn gl_update(ctx: g::GfxRefMut, state_ref: DriverRef, factory: &mut g::Factory) {
    let render = *ctx.0 as *mut RenderImpl<Res, Wrapper>;
    let render: &mut RenderImpl<Res, Wrapper> = unsafe { &mut *render };
    let state_ptr = *state_ref.0 as *const DriverState<Wrapper>;
    let state = unsafe { &*state_ptr };
    render.update(state, factory);
}

#[no_mangle]
pub extern "C" fn gl_draw(ctx: g::GfxRef, encoder: &mut Encoder) {
    let render = *ctx.0 as *const RenderImpl<Res, Wrapper>;
    let render: &RenderImpl<Res, Wrapper> = unsafe { &*render };
    render.draw(encoder);
}

impl<P: Pipe> RenderImpl<Res, P> {
    pub fn draw(&self, mut encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    pub fn update(&mut self, state: &DriverState<P>, factory: &mut g::Factory) {

        while let Some(msg) = state.pipe.try_recv::<api::DownResponse>().unwrap() {
            println!("=== {:?} ===", msg);
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
