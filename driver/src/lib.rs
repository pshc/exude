#![feature(box_syntax)]

#[macro_use]
extern crate g;
extern crate libc;
extern crate proto;

pub mod comms;
mod driver_abi;

use std::io::{self, ErrorKind, Write};
use std::mem;
use std::sync::Arc;

use libc::c_void;

use comms::{Pipe, Wrapper};
use driver_abi::DriverCallbacks;
use g::{DriverHandle, Encoder, Res, gfx};
use g::gfx::traits::FactoryExt;
use proto::api;

#[no_mangle]
pub extern "C" fn version() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn setup(cbs: *mut DriverCallbacks) -> DriverHandle {
    let pipe = Wrapper::new(cbs);
    let state = DriverState::new(pipe);
    let ptr = match state {
        Ok(arc) => Arc::into_raw(arc) as *const c_void,
        Err(e) => {
            let _ = writeln!(io::stderr(), "Driver setup: {}", e);
            std::ptr::null()
        }
    };
    DriverHandle(ptr)
}

#[no_mangle]
pub extern "C" fn teardown(handle: DriverHandle) -> *mut DriverCallbacks {
    assert!(!handle.0.is_null());
    let arc = unsafe { Arc::from_raw(handle.0 as *const DriverState<Wrapper>) };
    match Arc::try_unwrap(arc) {
        Ok(state) => {
            let wrapper: Wrapper = state.shutdown();
            wrapper.consume()
        }
        Err(_arc) => {
            println!("teardown: DriverHandle still held somewhere!");
            std::ptr::null_mut()
        }
    }
}

/// This is what we stash inside DriverHandle.
pub struct DriverState<P> {
    pipe: P,
}

impl<P> DriverState<P> {
    pub fn new(pipe: P) -> io::Result<Arc<Self>> {
        let state = DriverState { pipe };
        Ok(Arc::new(state))
    }

    pub fn shutdown(self) -> P {
        self.pipe
    }

    unsafe fn borrow(handle: DriverHandle) -> Arc<Self> {
        let arc = Arc::from_raw(handle.0 as *const DriverState<P>);
        let borrow = arc.clone();
        mem::forget(arc);
        borrow
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
    handle: DriverHandle,
    factory: &mut g::Factory,
    rtv: g::RenderTargetView,
) -> Option<g::GfxBox> {

    let state = unsafe { DriverState::<Wrapper>::borrow(handle) };
    match RenderImpl::<Res>::new(state.as_ref(), factory, rtv) {
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

pub struct RenderImpl<R: gfx::Resources> {
    slice: gfx::Slice<R>,
    pso: gfx::PipelineState<R, pipe::Meta>,
    data: pipe::Data<R>,
}

impl RenderImpl<g::Res> {
    pub fn new(
        _: &DriverState<Wrapper>,
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

        Ok(RenderImpl { slice, pso, data })
    }
}

#[no_mangle]
pub extern "C" fn gl_update(ctx: g::GfxRefMut, handle: DriverHandle) {
    let render: *mut RenderImpl<Res> = *ctx.0 as *mut RenderImpl<Res>;
    let render: &mut RenderImpl<Res> = unsafe { &mut *render };
    render.update(handle);
}

#[no_mangle]
pub extern "C" fn gl_draw(ctx: g::GfxRef, encoder: &mut Encoder) {
    let render = *ctx.0 as *const RenderImpl<Res>;
    let render: &RenderImpl<Res> = unsafe { &*render };
    render.draw(encoder);
    std::thread::sleep(std::time::Duration::from_millis(10));
}

impl RenderImpl<Res> {
    pub fn draw(&self, mut encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data)
    }

    pub fn update(&mut self, handle: DriverHandle) {
        let state = unsafe { DriverState::<Wrapper>::borrow(handle) };

        while let Some(msg) = state.pipe.try_recv::<api::DownResponse>().unwrap() {
            println!("=== {:?} ===", msg);
        }

        // update gpu state here...
    }
}

#[no_mangle]
pub extern "C" fn gl_cleanup(ctx: g::GfxBox) {
    let render = unsafe { Box::from_raw(ctx.consume() as *mut RenderImpl<Res>) };
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
