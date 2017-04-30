#![feature(box_syntax)]

extern crate bincode;
#[macro_use]
extern crate g;
extern crate libc;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod env;

use std::io::{self, Write};
use std::thread;

use g::gfx;
use g::gfx::traits::FactoryExt;
use g::gfx_device_gl;

#[no_mangle]
pub extern fn version() -> u32 {
    0
}

#[no_mangle]
pub extern fn driver(env: *mut env::DriverEnv) {
    let env = unsafe { Box::from_raw(env) };

    let _input = thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut line = String::new();
        loop {
            match env.try_recv::<env::DownResponse>() {
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
                env.send(&env::UpRequest::Ping(n)).unwrap();
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
        out: gfx::RenderTarget<env::ColorFormat> = "Target0",
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
    render_target: Box<gfx::handle::RenderTargetView<gfx_device_gl::Resources, env::ColorFormat>>)
    -> io::Result<Box<env::DrawGL>>
{
    let pso = factory.create_pipeline_simple(
        include_bytes!("shader/triangle_150.glslv"),
        include_bytes!("shader/triangle_150.glslf"),
        pipe::new()
    ).unwrap(); // xxx

    let (vertex_buffer, slice) = factory.create_vertex_buffer_with_slice(&TRIANGLE, ());
    let data = pipe::Data {
        vbuf: vertex_buffer,
        out: *render_target
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
pub extern fn gl_draw(data: &env::DrawGL, encoder: &mut gfx::Encoder<env::Res, env::Command>) {
    data.draw(encoder)
}

impl env::DrawGL for RenderImpl<env::Res, pipe::Meta> {
    fn draw(&self, mut encoder: &mut gfx::Encoder<env::Res, env::Command>) {
        encoder.draw(&self.slice, &self.pso, &self.data)
    }
}

#[no_mangle]
pub extern fn gl_cleanup(data: Box<env::DrawGL>) {
    drop(data);
    println!("cleaned up GL");
}
