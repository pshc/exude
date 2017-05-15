#![feature(box_syntax, drop_types_in_const)]
#![recursion_limit = "1024"]

extern crate digest;
#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate futures_cpupool;
#[macro_use]
extern crate g;
extern crate libloading;
extern crate proto;
#[macro_use]
extern crate rental;
extern crate sha3;
extern crate sodiumoxide;
extern crate tokio_core;
extern crate tokio_io;

mod basic;
#[path="../../server/src/common.rs"]
mod common;
mod connector;
#[path="../../driver/src/driver_abi.rs"]
mod driver_abi;
mod errors;
mod net;
mod receive;
mod render_loop;

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;
use std::sync::mpsc;
use std::thread;

use futures::{Future, future};
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;

use g::gfx::Device;
use g::gfx_text;
use g::gfx_window_glutin;
use g::glutin;

use common::OurFuture;
use errors::*;
use proto::handshake;
use render_loop::Engine;

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn main() {
    let addr = ([127, 0, 0, 1], 2001).into();

    if let Err(e) = client(addr) {
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

fn client(server_addr: SocketAddr) -> Result<()> {

    // for sending a new driver from net to draw thread
    let (update_tx, update_rx) = mpsc::channel::<DriverUpdate>();

    let _net_thread = thread::spawn(
        move || {
            let mut core = Core::new().expect("net: core");
            let handle = core.handle();

            let client = TcpStream::connect(&server_addr, &handle)
                .then(|res| res.chain_err(|| format!("couldn't connect to server")))
                .and_then(
                    |sock| {
                        let (reader, writer) = sock.split();

                        let greeting = {
                            let cached_driver = None;
                            let hello = handshake::Hello(cached_driver);
                            common::write_bincoded(writer, &hello).and_then(|(w, _)| Ok(w))
                        };

                        let welcome = receive::fetch_driver(reader);

                        welcome
                            .join(greeting)
                            .and_then(
                                move |((r, info, path), w)| -> OurFuture<_> {
                                    println!("driver {}", info.digest.short_hex());

                                    let (driver_tx, driver_rx) = mpsc::channel();
                                    let (tx, rx) = futures::sync::mpsc::unbounded();
                                    let comms = connector::DriverComms { rx: driver_rx, tx: tx };
                                    let net_comms = net::Comms { tx: driver_tx, rx: rx };

                                    // inform the draw thread about our new driver
                                    if update_tx.send((path, box comms)).is_err() {
                                        return box future::err(ErrorKind::BrokenComms.into());
                                    }

                                    net_comms.handle(r, w)
                                }
                            )
                    }
                );

            match core.run(client) {
                Ok((_r, _w)) => println!("net: donezo"),
                Err(e) => errors::display_net_thread_error(e).expect("net: stderr?"),
            }
        }
    );

    let events_loop = g::EventsLoop::new();
    let builder = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, main_color, main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, g::DepthFormat>(builder, &events_loop);

    let mut engine = Hot::new(&mut factory, main_color, main_depth, update_rx)?;

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

        engine.update(&(), &mut factory)?;
        engine.draw(&mut encoder)?;

        encoder.flush(&mut device);
        window.swap_buffers().chain_err(|| "swapping buffers")?;
        device.cleanup();
    }

    drop(engine); // waits for driver cleanup
    Ok(())
}

/// Our driver-loading Engine.
struct Hot {
    basic_vis: basic::Renderer<g::Res>,
    driver: Option<(connector::Driver, g::GfxBox)>,
    main_color: g::RenderTargetView,
    main_depth: g::DepthStencilView,
    text: gfx_text::Renderer<g::Res, g::Factory>,
    update_rx: mpsc::Receiver<DriverUpdate>,
}

type DriverUpdate = (PathBuf, Box<connector::DriverComms>);

impl Hot {
    fn new(
        factory: &mut g::Factory,
        rtv: g::RenderTargetView,
        dsv: g::DepthStencilView,
        update_rx: mpsc::Receiver<DriverUpdate>,
    ) -> Result<Self> {

        let basic_vis = basic::Renderer::new(factory, rtv.clone())
            .chain_err(|| "couldn't set up basic renderer")?;
        let text = gfx_text::new(factory.clone())
            .with_size(30)
            .build()
            .map_err(ErrorKind::Text)?;
        Ok(
            Hot {
                basic_vis,
                driver: None,
                main_color: rtv,
                main_depth: dsv,
                text,
                update_rx,
            }
        )
    }
}

impl Engine<g::Res> for Hot {
    type CommandBuffer = g::Command;
    type Factory = g::Factory;
    // connector::Driver holds the driver's state for us
    type State = ();

    fn draw(&mut self, mut encoder: &mut g::Encoder) -> Result<()> {
        encoder.clear(&self.main_color, BLACK);

        if let Some((ref driver, ref ctx)) = self.driver {
            driver.draw(ctx.borrow(), encoder);
            self.text.add("Active", [10, 10], WHITE);
        } else {
            self.basic_vis.draw(encoder);
            self.text.add("Loading...", [10, 10], WHITE);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        self.text
            .draw(&mut encoder, &self.main_color)
            .map_err(|t| ErrorKind::Text(t).into())
    }

    fn update(&mut self, _: &(), factory: &mut g::Factory) -> Result<()> {

        if let Ok((path, comms)) = self.update_rx.try_recv() {
            println!("Loading driver...");
            io::stdout().flush().expect("stderr");

            match connector::load(&path, comms) {
                Ok(new_driver) => {
                    // Is there already a driver running?
                    if let Some((old, old_ctx)) = self.driver.take() {
                        // We should definitely do this asynchronously.
                        old.gfx_cleanup(old_ctx);
                        println!("Waiting for old driver...");
                        old.join();
                    } else {
                        println!("Setting up driver...");
                    }
                    io::stdout().flush().expect("stderr");

                    match new_driver.gfx_setup(factory, self.main_color.clone()) {
                        Some(ctx) => {
                            self.driver = Some((new_driver, ctx));
                            println!("Driver OK!");
                        }
                        None => {
                            println!("Waiting for failed driver...");
                            new_driver.join();
                        }
                    }
                }
                Err(e) => {
                    println!("Failed: {}", e);
                    debug_assert!(false, "{:?}", e);
                }
            }
        }

        if let Some((ref driver, ref mut ctx)) = self.driver {
            driver.update(ctx.borrow_mut(), factory);
        } else {
            self.basic_vis.update(factory, &self.main_color)?;
        }
        Ok(())
    }
}

impl Drop for Hot {
    fn drop(&mut self) {
        if let Some((driver, ctx)) = self.driver.take() {
            driver.gfx_cleanup(ctx);
            println!("Waiting for driver cleanup...");
            driver.join();
        }
    }
}
