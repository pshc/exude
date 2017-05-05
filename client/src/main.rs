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
mod receive;

use std::io::{self, ErrorKind, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use futures::{Future, Stream};
use gfx::Device;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};

use g::gfx;
use g::gfx_text;
use g::gfx_window_glutin;
use g::glutin;
use g::GlInterface;

use common::IoFuture;
use proto::handshake;

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();

    // for sending a new driver from net to draw thread
    let (update_tx, update_rx) = mpsc::channel::<(PathBuf, Box<connector::DriverComms>)>();

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
                                let net_comms = NetComms { tx: driver_tx, rx: rx };

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

    type DepthFormat = gfx::format::Depth;
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    let builder = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, mut main_color, mut main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, DepthFormat>(builder);

    let mut encoder: gfx::Encoder<_, _> = factory.create_command_buffer().into();

    let mut basic_vis = basic::Renderer::new(&mut factory, main_color.clone())
        .ok()
        .map(|v| (v, 0.0));
    let mut text = gfx_text::new(factory.clone()).with_size(30).unwrap();

    let mut driver = None;

    'main: loop {
        if let Ok((path, comms)) = update_rx.try_recv() {
            basic_vis = None;

            println!("loading driver...");
            io::stdout().flush().unwrap();
            let loaded = connector::load(&path, comms).unwrap();
            println!("driver setup!");
            io::stdout().flush().unwrap();

            let ctx = loaded.setup(&mut factory, main_color.clone()).unwrap();
            driver = Some((loaded, ctx));
        }

        for event in window.poll_events() {
            use glutin::VirtualKeyCode::*;

            match event {
                glutin::Event::KeyboardInput(_, _, Some(Escape)) |
                glutin::Event::KeyboardInput(_, _, Some(Grave)) |
                glutin::Event::Closed => break 'main,

                glutin::Event::Resized(_w, _h) => {
                    gfx_window_glutin::update_views(&window, &mut main_color, &mut main_depth);
                }
                _ => {}
            }
            // we should probably forward the events to driver?
        }


        encoder.clear(&main_color, BLACK);
        if let Some((ref driver, ref ctx)) = driver {
            driver.draw(&**ctx, &mut encoder);
            text.add("Active", [10, 10], WHITE);
        } else {
            if let Some((ref mut vis, ref mut progress)) = basic_vis {
                *progress += 0.01;
                let _res = vis.update(&mut factory, *progress);
                debug_assert_eq!(_res, Ok(()));
                vis.draw(&mut encoder);
            }
            text.add("Loading...", [10, 10], WHITE);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        text.draw(&mut encoder, &main_color).unwrap();

        encoder.flush(&mut device);
        window.swap_buffers().unwrap();
        device.cleanup();
    }

    if let Some((driver, ctx)) = driver {
        driver.cleanup(ctx);
        println!("Waiting on driver's IO thread...");
        driver.io_join();
        println!("OK! Unloading driver.");
    }
}

struct NetComms {
    tx: mpsc::Sender<Box<[u8]>>,
    rx: futures::sync::mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl NetComms {
    fn handle<R, W>(self, r: R, w: W) -> IoFuture<(R, W)>
    where
        R: AsyncRead + 'static,
        W: AsyncWrite + 'static,
    {
        let NetComms { tx, rx } = self;

        // todo: read more than one message
        let read = common::read_with_length(r).and_then(
            move |(r, vec)| {
                tx.send(vec.into_boxed_slice())
                    .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "core: done reading"),)
                    .map(|_| r)
            },
        );

        let write = rx
        .map_err(|()| io::Error::new(ErrorKind::BrokenPipe, "core: done writing"))
        .fold(w, |w, msg| {
            common::write_with_length(w, msg).map(|(w, _)| w)
        });

        box read.join(write)
    }
}
