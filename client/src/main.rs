#![feature(box_syntax, drop_types_in_const)]

extern crate digest;
extern crate futures;
extern crate futures_cpupool;
#[macro_use]
extern crate g;
extern crate libc;
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
mod net;
mod receive;
mod render_loop;

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use futures::Future;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;

use g::gfx_text;
use g::gfx_window_glutin;
use g::glutin;
use g::GlInterface;

use proto::handshake;

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();

    // for sending a new driver from net to draw thread
    let (update_tx, update_rx) = mpsc::channel::<DriverUpdate>();

    let _net_thread = thread::spawn(
        move || {
            let mut core = Core::new().unwrap();
            let handle = core.handle();

            let client = TcpStream::connect(&addr, &handle).and_then(
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
                            move |((r, info, path), w)| {
                                println!("driver {}", info.digest.short_hex());

                                let (driver_tx, driver_rx) = mpsc::channel();
                                let (tx, rx) = futures::sync::mpsc::unbounded();
                                let comms = connector::DriverComms { rx: driver_rx, tx: tx };
                                let net_comms = net::Comms { tx: driver_tx, rx: rx };

                                // inform the draw thread about our new driver
                                update_tx.send((path, box comms)).unwrap();

                                box net_comms.handle(r, w).map(|_| println!("net: donezo"))
                            },
                        )
                },
            );

            core.run(client).unwrap();
        },
    );

    let builder = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, device, mut factory, main_color, main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, g::DepthFormat>(builder);

    let mut engine = Hot::new(
        &mut factory,
        main_color.clone(),
        main_depth.clone(),
        update_rx,
    )
            .unwrap();

    let encoder = factory.create_command_buffer().into();

    render_loop::render_loop(window, device, factory, encoder, &mut engine);

    drop(engine); // waits for driver cleanup
}

/// Our driver-loading Engine.
struct Hot {
    basic_vis: basic::Renderer<g::Res>,
    driver: Option<(connector::Driver, Box<g::GlCtx>)>,
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
    ) -> io::Result<Self> {

        let basic_vis = basic::Renderer::new(factory, rtv.clone())?;
        let text = gfx_text::new(factory.clone())
            .with_size(30)
            .build()
            .unwrap(); // xxx
        Ok(
            Hot {
                basic_vis,
                driver: None,
                main_color: rtv,
                main_depth: dsv,
                text,
                update_rx,
            },
        )
    }
}

impl render_loop::Engine<g::Res> for Hot {
    type CommandBuffer = g::Command;
    type Factory = g::Factory;

    fn draw(&mut self, encoder: &mut g::Encoder) {
        encoder.clear(&self.main_color, BLACK);

        if let Some((ref driver, ref ctx)) = self.driver {
            driver.draw(&**ctx, encoder);
            self.text.add("Active", [10, 10], WHITE);
        } else {
            self.basic_vis.draw(encoder);
            self.text.add("Loading...", [10, 10], WHITE);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn resize(&mut self, window: &glutin::Window) {
        gfx_window_glutin::update_views(&window, &mut self.main_color, &mut self.main_depth);
    }

    fn update(&mut self, factory: &mut g::Factory) {

        if let Ok((path, comms)) = self.update_rx.try_recv() {
            println!("Loading driver...");
            io::stdout().flush().unwrap();

            match connector::load(&path, comms) {
                Ok(new_driver) => {
                    // Is there already a driver running?
                    if let Some((old, old_ctx)) = self.driver.take() {
                        // We should definitely do this asynchronously.
                        old.cleanup(old_ctx);
                        println!("Waiting for old driver...");
                        old.io_join();
                    } else {
                        println!("Setting up driver...");
                    }
                    io::stdout().flush().unwrap();

                    match new_driver.setup(factory, self.main_color.clone()) {
                        Ok(ctx) => {
                            self.driver = Some((new_driver, ctx));
                            println!("Driver OK!");
                        }
                        Err(e) => {
                            println!("Driver: {}", e);
                            println!("Waiting for failed driver...");
                            new_driver.io_join();
                        }
                    }
                }
                Err(e) => {
                    println!("Failed: {}", e);
                    debug_assert!(false, "{:?}", e);
                }
            }
        }

        if self.driver.is_none() {
            self.basic_vis.update(factory, &self.main_color);
        }
    }
}

impl Drop for Hot {
    fn drop(&mut self) {
        if let Some((driver, ctx)) = self.driver.take() {
            driver.cleanup(ctx);
            println!("Waiting on driver's IO thread...");
            driver.io_join();
        }
    }
}
