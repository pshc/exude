#![feature(box_syntax)]

extern crate client;
extern crate driver;
extern crate futures;
extern crate g;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;

use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use futures::Future;
use g::gfx::Device;
use g::gfx_text;
use g::gfx_window_glutin;
use g::glutin;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;

use client::net;
use client::common::{self, IoFuture};
use client::render_loop::{self, Engine};
use proto::{Bincoded, Digest, handshake};
use proto::bincoded;
use proto::serde::{Deserialize, Serialize};

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();

    let io_comms;
    let net_comms;
    {
        let (driver_tx, driver_rx) = mpsc::channel();
        let (tx, rx) = futures::sync::mpsc::unbounded();
        io_comms = StaticComms { rx: driver_rx, tx: tx };
        net_comms = net::Comms { tx: driver_tx, rx: rx };
    }

    let _net_thread = thread::spawn(
        move || {
            let mut core = Core::new().unwrap();
            let handle = core.handle();

            let client = TcpStream::connect(&addr, &handle).and_then(
                |sock| {
                    let (reader, writer) = sock.split();

                    let greeting = {
                        // TEMP we should have like `handshake::Hello::RequireVersion` to indicate our intent?
                        let cached_driver = Some(Digest::zero()); // TEMP
                        let hello = handshake::Hello(cached_driver);
                        common::write_bincoded(writer, &hello).and_then(|(w, _)| Ok(w))
                    };

                    let welcome = common::read_bincoded(reader).and_then(
                        |(reader, welcome)| {
                            match welcome {
                                handshake::Welcome::Current => Ok(reader),
                                _ => Err(io::Error::new(ErrorKind::Other, "client too outdated for server")),
                            }
                        }
                    );

                    welcome
                        .join(greeting)
                        .and_then(
                            move |(r, w)| -> IoFuture<_> {
                                box net_comms.handle(r, w).map(|_| println!("net: donezo"))
                            },
                        )
                },
            );

            core.run(client).unwrap();
        },
    );

    let driver_io = thread::Builder::new()
        .name("driver_io".into())
        .spawn(move || driver::io_thread(&io_comms))
        .unwrap();

    let builder = glutin::WindowBuilder::new()
        .with_title("Standalone".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, main_color, main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, g::DepthFormat>(builder);

    let mut engine = Oneshot::new(&mut factory, main_color, main_depth).unwrap();

    let mut encoder = factory.create_command_buffer().into();

    'main: loop {
        for event in window.poll_events() {
            if render_loop::should_quit(&event) {
                break 'main;
            }
            if let g::winit::Event::Resized(_w, _h) = *&event {
                gfx_window_glutin::update_views(
                    &window,
                    &mut engine.main_color,
                    &mut engine.main_depth,
                );
            }
        }

        engine.update(&mut factory);
        engine.draw(&mut encoder);

        encoder.flush(&mut device);
        window.swap_buffers().unwrap();
        device.cleanup();
    }

    engine.cleanup();
    driver_io.join().unwrap();
}

struct StaticComms {
    pub rx: mpsc::Receiver<Box<[u8]>>,
    pub tx: futures::sync::mpsc::UnboundedSender<Box<[u8]>>,
}

impl driver::comms::Pipe for StaticComms {
    fn send<T: Serialize>(&self, msg: &T) -> io::Result<()> {
        // so many copies... ugh!
        let bin = Bincoded::new(msg)?;
        // todo we should go directly to Box<[u8]>
        let vec: Vec<u8> = bin.into();
        assert!(vec.len() <= ::std::i32::MAX as usize);
        self.tx
            .send(vec.into_boxed_slice())
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "send: pipe broken"),)
    }

    fn try_recv<T>(&self) -> io::Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>,
    {
        match self.rx.try_recv() {
            Ok(boxed_slice) => {
                let val = bincoded::deserialize_exact(boxed_slice)?;
                Ok(Some(val))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                let err = io::Error::new(ErrorKind::BrokenPipe, "try_recv: pipe broken");
                Err(err)
            }
        }
    }
}

/// Our statically linked Engine.
struct Oneshot {
    ctx: Box<g::GlCtx>,
    main_color: g::RenderTargetView,
    main_depth: g::DepthStencilView,
    text: gfx_text::Renderer<g::Res, g::Factory>,
}

impl Oneshot {
    fn new(
        factory: &mut g::Factory,
        rtv: g::RenderTargetView,
        dsv: g::DepthStencilView,
    ) -> io::Result<Self> {

        let ctx = driver::gl_setup(factory, rtv.clone())?;

        let text = gfx_text::new(factory.clone())
            .with_size(30)
            .build()
            .unwrap(); // xxx
        Ok(Oneshot { ctx, main_color: rtv, main_depth: dsv, text })
    }

    fn cleanup(self) {
        driver::gl_cleanup(self.ctx);
    }
}

impl Engine<g::Res> for Oneshot {
    type CommandBuffer = g::Command;
    type Factory = g::Factory;

    fn draw(&mut self, encoder: &mut g::Encoder) {
        encoder.clear(&self.main_color, BLACK);
        driver::gl_draw(&*self.ctx, encoder);
        self.text.add("Oneshot", [10, 10], WHITE);
    }

    fn update(&mut self, _: &mut g::Factory) {}
}
