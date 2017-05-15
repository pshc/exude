use g::gfx;
use g::WindowEvent;

/// Called upon by the render loop.
pub trait Engine<R: gfx::Resources> {
    type CommandBuffer: gfx::CommandBuffer<R>;
    type Factory: gfx::Factory<R>;
    type State;

    fn draw(&mut self, &mut gfx::Encoder<R, Self::CommandBuffer>);
    fn update(&mut self, &Self::State, &mut Self::Factory);
}

pub fn should_quit(event: &WindowEvent) -> bool {
    use g::ElementState::Pressed;
    use g::VirtualKeyCode::{Escape, Grave};

    match *event {
        WindowEvent::KeyboardInput(Pressed, _, Some(Escape), _) |
        WindowEvent::KeyboardInput(Pressed, _, Some(Grave), _) |
        WindowEvent::Closed => true,
        _ => false,
    }
}
