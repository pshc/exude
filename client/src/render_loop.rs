use g::gfx;
use g::Event;

/// Called upon by the render loop.
pub trait Engine<R: gfx::Resources> {
    type CommandBuffer: gfx::CommandBuffer<R>;
    type Factory: gfx::Factory<R>;
    type State;

    fn draw(&mut self, &mut gfx::Encoder<R, Self::CommandBuffer>);
    fn update(&mut self, &Self::State, &mut Self::Factory);
}

pub fn should_quit(event: &Event) -> bool {
    use g::VirtualKeyCode::{Escape, Grave};

    match *event {
        Event::KeyboardInput(_, _, Some(Escape)) |
        Event::KeyboardInput(_, _, Some(Grave)) |
        Event::Closed => true,
        _ => false,
    }
}
