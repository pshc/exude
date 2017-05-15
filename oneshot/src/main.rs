#![feature(box_syntax)]

extern crate client;
extern crate driver;
#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate g;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;

use std::io::{self, Write};
use std::net::SocketAddr;
use std::process;
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
use client::common::{self, OurFuture};
use client::render_loop::{self, Engine};
use driver::{DriverState, RenderImpl};
use proto::{Bincoded, Digest, handshake};
use proto::bincoded;
use proto::serde::{Deserialize, Serialize};

mod errors {
    error_chain! {
        links {
            Client(::client::Error, ::client::ErrorKind);
            Driver(::driver::Error, ::driver::ErrorKind);
        }
    }
}
use errors::*;

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();

    if let Err(e) = oneshot(addr) {
        let stderr = io::stderr();
        let oops = "couldn't write to stderr";
        let mut log = stderr.lock();
        if let Some(backtrace) = e.backtrace() {
            writeln!(log, "\n{:?}\n", backtrace).expect(oops);
        }
        writeln!(log, "error: {}", e).expect(oops);
        for e in e.iter().skip(1) {
            writeln!(log, "caused by: {}", e).expect(oops);
        }
        drop(log);
        process::exit(1);
    }
}

fn oneshot(server_addr: SocketAddr) -> Result<()> {

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
            let mut core = Core::new().expect("net: core");
            let handle = core.handle();

            let client = TcpStream::connect(&server_addr, &handle)
                .then(|res| {
                    client::ResultExt::chain_err(
                        res,
                        || format!("couldn't connect to {}", server_addr)
                    )
                })
                .and_then(
                |sock| -> OurFuture<_> {
                    let (reader, writer) = sock.split();

                    let greeting = {
                        // TEMP we should have like `handshake::Hello::RequireVersion` to indicate our intent?
                        let cached_driver = Some(Digest::zero()); // TEMP
                        let hello = handshake::Hello(cached_driver);
                        common::write_bincoded(writer, &hello)
                            .and_then(|(w, _)| Ok(w))
                    };

                    let welcome = common::read_bincoded(reader)
                        .and_then(
                        |(reader, welcome)| {
                            match welcome {
                                handshake::Welcome::Current => Ok(reader),
                                _ => bail!("client too outdated for server"),
                            }
                        }
                    );

                    box welcome
                        .join(greeting)
                        .and_then(
                            move |(r, w)| -> OurFuture<_> {
                                box net_comms.handle(r, w)
                            },
                        )
                },
            );

            match core.run(client) {
                Ok((_r, _w)) => println!("net: donezo"),
                Err(e) => client::errors::display_net_thread_error(e).expect("net: stderr?"),
            }
        },
    );

    let state: DriverState<StaticComms> = DriverState::new(io_comms);

    let events_loop = g::EventsLoop::new();
    let builder = glutin::WindowBuilder::new()
        .with_title("Standalone".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, main_color, main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, g::DepthFormat>(builder, &events_loop);

    let mut engine = Oneshot::new(&state, &mut factory, main_color, main_depth)?;

    let mut encoder = factory.create_command_buffer().into();

    let mut alive = true;
    loop {
        events_loop.poll_events(
            |event| {
                let g::Event::WindowEvent { ref event, .. } = event;
                if render_loop::should_quit(&event) {
                    alive = false;
                    return;
                }
                if let g::WindowEvent::Resized(_w, _h) = *event {
                    gfx_window_glutin::update_views(
                        &window,
                        &mut engine.main_color,
                        &mut engine.main_depth,
                    );
                }
            }
        );
        if !alive {
            break;
        }

        engine.update(&state, &mut factory)?;
        engine.draw(&mut encoder)?;

        encoder.flush(&mut device);
        window.swap_buffers().chain_err(|| "swapping buffers")?;
        device.cleanup();
    }

    drop(engine);
    let _: StaticComms = state.shutdown();

    Ok(())
}

struct StaticComms {
    pub rx: mpsc::Receiver<Box<[u8]>>,
    pub tx: futures::sync::mpsc::UnboundedSender<Box<[u8]>>,
}

impl driver::comms::Pipe for StaticComms {
    fn send<T: Serialize>(&self, msg: &T) -> driver::Result<()> {
        // so many copies... ugh!
        let bin = Bincoded::new(msg)?;
        // todo we should go directly to Box<[u8]>
        let vec: Vec<u8> = bin.into();
        assert!(vec.len() <= ::std::i32::MAX as usize);
        let res = self.tx.send(vec.into_boxed_slice());
        driver::ResultExt::chain_err(res, || format!("couldn't send message"))
    }

    fn try_recv<T>(&self) -> driver::Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>,
    {
        match self.rx.try_recv() {
            Ok(boxed_slice) => {
                let val = bincoded::deserialize_exact(boxed_slice)?;
                Ok(Some(val))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(driver::ErrorKind::BrokenComms.into()),
        }
    }
}

/// Our statically linked Engine.
struct Oneshot {
    render: RenderImpl<g::Res, StaticComms>,
    main_color: g::RenderTargetView,
    main_depth: g::DepthStencilView,
    text: gfx_text::Renderer<g::Res, g::Factory>,
}

impl Oneshot {
    fn new(
        state: &DriverState<StaticComms>,
        factory: &mut g::Factory,
        rtv: g::RenderTargetView,
        dsv: g::DepthStencilView,
    ) -> Result<Self> {

        let render = RenderImpl::new(state, factory, rtv.clone())?;

        let text = gfx_text::new(factory.clone())
            .with_size(30)
            .build()
            .map_err(|e| -> client::Error { client::ErrorKind::Text(e).into() })?;
        Ok(Oneshot { render, main_color: rtv, main_depth: dsv, text })
    }
}

impl Engine<g::Res> for Oneshot {
    type CommandBuffer = g::Command;
    type Factory = g::Factory;
    type State = DriverState<StaticComms>;

    fn draw(&mut self, encoder: &mut g::Encoder) -> client::Result<()> {
        encoder.clear(&self.main_color, BLACK);
        self.render.draw(encoder);
        self.text.add("Oneshot", [10, 10], WHITE);
        self.text
            .draw(encoder, &self.main_color)
            .map_err(|e| client::ErrorKind::Text(e).into())
    }

    fn update(
        &mut self,
        state: &DriverState<StaticComms>,
        factory: &mut g::Factory,
    ) -> client::Result<()> {
        self.render.update(state, factory);
        Ok(())
    }
}
