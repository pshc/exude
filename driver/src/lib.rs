#![feature(box_syntax)]

#[macro_use]
extern crate g;
extern crate libc;
extern crate proto;
extern crate serde;

mod driver_abi;
mod wrapper;

use std::io::{self, ErrorKind, Write};
use std::thread;

use g::{GlCtx, Encoder, Res, gfx};
use g::gfx::traits::FactoryExt;
use proto::api;

#[no_mangle]
pub extern fn version() -> u32 {
    0
}

#[no_mangle]
pub extern fn driver(cbs: *mut driver_abi::DriverCallbacks) {
    let pipe = wrapper::Pipe::wrap(cbs);

    let _input = thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut line = String::new();
        loop {
            match pipe.try_recv::<api::DownResponse>() {
                Ok(None) => (),
                Ok(Some(resp)) => {
                    println!("=== {:?} ===", resp);
                }
                Err(_) => println!("driver: cannot read")
            }

            print!("> ");
            let _res = stdout.flush();
            debug_assert!(_res.is_ok());

            line.clear();
            let line = match stdin.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => &line,
                Err(e) => {
                    println!("{}", e);
                    break
                }
            };

            let line = line.trim();
            if line == "q" {
                break
            }

            if let Ok(n) = line.parse::<u32>() {
                println!("n: {}", n);
                pipe.send(&api::UpRequest::Ping(n)).unwrap();
            }
        }

        drop(pipe);
    });
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
    Vertex { pos: [ -0.5, -0.5 ], color: [1.0, 0.0, 0.0] },
    Vertex { pos: [  0.5, -0.5 ], color: [0.0, 1.0, 0.0] },
    Vertex { pos: [  0.0,  0.5 ], color: [0.0, 0.0, 1.0] }
];

#[no_mangle]
pub extern fn gl_setup(factory: &mut g::Factory,
                       render_target: g::RenderTargetView)
                       -> io::Result<Box<g::GlCtx>>
{
    let pso = factory.create_pipeline_simple(
        include_bytes!("shader/triangle_150.glslv"),
        include_bytes!("shader/triangle_150.glslf"),
        pipe::new()
    ).map_err(|e| io::Error::new(ErrorKind::Other, e))?;

    let (vertex_buffer, slice) = factory.create_vertex_buffer_with_slice(&TRIANGLE, ());
    let data = pipe::Data {
        vbuf: vertex_buffer,
        out: render_target
    };

    Ok(box RenderImpl {
        slice: slice,
        pso: pso,
        data: data
    })
}

struct RenderImpl<R: gfx::Resources, M> {
    slice: gfx::Slice<R>,
    pso: gfx::PipelineState<R, M>,
    data: pipe::Data<R>,
}

#[no_mangle]
pub extern fn gl_draw(ctx: &GlCtx, encoder: &mut Encoder) {
    ctx.draw(encoder);
    std::thread::sleep(std::time::Duration::from_millis(10));
}

impl GlCtx for RenderImpl<Res, pipe::Meta> {
    fn draw(&self, mut encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data)
    }
}

#[no_mangle]
pub extern fn gl_cleanup(ctx: Box<GlCtx>) {
    drop(ctx);
    println!("cleaned up GL");
}

#[allow(dead_code)]
fn check_gl_types() {
    let _: g::GlSetupFn = gl_setup;
    let _: g::GlDrawFn = gl_draw;
    let _: g::GlCleanupFn = gl_cleanup;
}
