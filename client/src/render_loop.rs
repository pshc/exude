use g::gfx;
use g::WindowEvent;

use errors::*;

/// Called upon by the render loop.
pub trait Engine<R: gfx::Resources> {
    type CommandBuffer: gfx::CommandBuffer<R>;
    type Factory: gfx::Factory<R>;
    type State;

    fn draw(&mut self, &mut gfx::Encoder<R, Self::CommandBuffer>) -> Result<()>;
    fn update(&mut self, &mut Self::State, &mut Self::Factory) -> Result<()>;
}

pub fn should_quit(event: &WindowEvent) -> bool {
    use g::ElementState::Pressed;
    use g::VirtualKeyCode::{Escape, Grave};

    match *event {
        WindowEvent::KeyboardInput { input, .. } if input.state == Pressed => {
            match input.virtual_keycode {
                Some(Escape) | Some(Grave) => true,
                _ => false
            }
        }
        WindowEvent::Closed => true,
        _ => false,
    }
}
