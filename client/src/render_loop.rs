use g::gfx;
use g::glutin;

/// Called upon by the render loop.
pub trait Engine<R: gfx::Resources> {
    type CommandBuffer: gfx::CommandBuffer<R>;
    type Factory: gfx::Factory<R>;

    fn draw(&mut self, &mut gfx::Encoder<R, Self::CommandBuffer>);
    fn resize(&mut self, &glutin::Window);
    fn update(&mut self, &mut Self::Factory);
}

pub fn render_loop<C, D, E, F, R>(
    window: glutin::Window,
    mut device: D,
    mut factory: F,
    mut encoder: gfx::Encoder<R, C>,
    mut engine: &mut E,
) where
    C: gfx::CommandBuffer<R>,
    D: gfx::Device<Resources = R, CommandBuffer = C>,
    E: Engine<R, CommandBuffer = C, Factory = F>,
    F: gfx::Factory<R>,
    R: gfx::Resources,
{
    'main: loop {
        for event in window.poll_events() {
            use self::glutin::VirtualKeyCode::*;

            match event {
                glutin::Event::KeyboardInput(_, _, Some(Escape)) |
                glutin::Event::KeyboardInput(_, _, Some(Grave)) |
                glutin::Event::Closed => break 'main,

                glutin::Event::Resized(_w, _h) => {
                    engine.resize(&window);
                }
                _ => {}
            }
            // we should probably forward the events to driver?
        }

        engine.update(&mut factory);

        engine.draw(&mut encoder);

        encoder.flush(&mut device);
        window.swap_buffers().unwrap();
        device.cleanup();
    }
}
