#![feature(box_syntax)]

#[macro_use]
extern crate g;
extern crate libc;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod env;
#[path="../../proto/mod.rs"]
mod proto;
mod wrapper;

use std::io::{self, ErrorKind, Write};
use std::thread;

use g::{DrawGL, Encoder, Res, gfx};
use g::gfx::traits::FactoryExt;
use g::gfx_device_gl;
use proto::api;

#[no_mangle]
pub extern fn version() -> u32 {
    0
}

#[no_mangle]
pub extern fn driver(env: *mut env::DriverEnv) {
    let env = wrapper::EnvWrapper::wrap(env);

    let _input = thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut line = String::new();
        loop {
            match env.try_recv::<api::DownResponse>() {
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
                env.send(&api::UpRequest::Ping(n)).unwrap();
            }
        }

        drop(env);
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
pub extern fn gl_setup(
    factory: &mut gfx_device_gl::Factory,
    render_target: g::RenderTargetView)
    -> io::Result<Box<g::DrawGL>>
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
pub extern fn gl_draw(data: &DrawGL, encoder: &mut Encoder) {
    data.draw(encoder);
    std::thread::sleep(std::time::Duration::from_millis(10));
}

impl DrawGL for RenderImpl<Res, pipe::Meta> {
    fn draw(&self, mut encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data)
    }
}

#[no_mangle]
pub extern fn gl_cleanup(data: Box<DrawGL>) {
    drop(data);
    println!("cleaned up GL");
}

#[allow(dead_code)]
fn check_gl_types() {
    let _: g::GlSetupFn = gl_setup;
    let _: g::GlDrawFn = gl_draw;
    let _: g::GlCleanupFn = gl_cleanup;
}
