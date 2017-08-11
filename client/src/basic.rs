use g::{self, ColorFormat, Encoder, RenderTargetView, Res};
use g::gfx::{self, IntoIndexBuffer};
use g::gfx::traits::{Factory, FactoryExt};

use errors::*;

gfx_defines! {
    vertex Vertex {
        pos: [f32; 2] = "a_Pos",
        color: [f32; 3] = "a_Color",
    }

    pipeline basic {
        vbuf: gfx::VertexBuffer<Vertex> = (),
        out: gfx::RenderTarget<ColorFormat> = "Target0",
    }
}

// gradient colors
const LEFT: [f32; 3] = [1.0, 0.0, 0.3];
const RIGHT: [f32; 3] = [0.3, 0.0, 1.0];

pub struct Renderer<R: gfx::Resources> {
    slice: gfx::Slice<R>,
    pso: gfx::PipelineState<R, basic::Meta>,
    data: basic::Data<R>,
    progress: f32,
}

impl Renderer<Res> {
    pub fn new(factory: &mut g::Factory, render_target: RenderTargetView) -> Result<Self> {
        let pso = factory
            .create_pipeline_simple(VERTEX_SHADER, FRAGMENT_SHADER, basic::new())
            .chain_err(|| "graphics pipeline")?;

        let indices = [0u16, 1, 2, 2, 1, 3].into_index_buffer(factory);
        let vertex_buffer = factory
            .create_buffer(
                4,
                gfx::buffer::Role::Vertex,
                gfx::memory::Usage::Upload,
                gfx::Bind::empty(),
            )
            .chain_err(|| "creating vertex buffer")?;
        {
            let mut vbuf = factory
                .write_mapping(&vertex_buffer)
                .chain_err(|| "setting up vertex buffer")?;
            vbuf[0] = Vertex { pos: [-1.0, -0.25], color: LEFT };
            vbuf[1] = Vertex { pos: [-1.0, 0.25], color: LEFT };
            vbuf[2] = Vertex { pos: [-1.0, -0.25], color: RIGHT };
            vbuf[3] = Vertex { pos: [-1.0, 0.25], color: RIGHT };
        }
        let slice = gfx::Slice {
            start: 0,
            end: 6,
            base_vertex: 0,
            instances: None,
            buffer: indices,
        };
        let data = basic::Data { vbuf: vertex_buffer, out: render_target };

        Ok(Renderer { slice, pso, data, progress: 0.0 })
    }

    pub fn update(&mut self, factory: &mut g::Factory, _: &g::RenderTargetView) -> Result<()> {
        self.progress += 0.01;
        let width = self.progress * 2.0 - 1.0;
        let mut vbuf = factory
            .write_mapping(&self.data.vbuf)
            .chain_err(|| "writing to vertex buffer")?;
        vbuf[2] = Vertex { pos: [width, -0.25], color: RIGHT };
        vbuf[3] = Vertex { pos: [width, 0.25], color: RIGHT };
        Ok(())
    }

    pub fn draw(&self, encoder: &mut Encoder) {
        encoder.draw(&self.slice, &self.pso, &self.data);
    }
}

static VERTEX_SHADER: &[u8] = b"
#version 150 core

in vec2 a_Pos;
in vec3 a_Color;
out vec4 v_Color;

void main() {
    v_Color = vec4(a_Color, 1.0);
    gl_Position = vec4(a_Pos, 0.0, 1.0);
}
";

static FRAGMENT_SHADER: &[u8] = b"
#version 150 core

in vec4 v_Color;
out vec4 Target0;

void main() {
    Target0 = v_Color;
}
";
