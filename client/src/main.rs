#![feature(box_syntax, drop_types_in_const)]
#![recursion_limit = "1024"]
#![allow(unused_doc_comment)] // temp until error_chain updated

extern crate digest;
#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate futures_cpupool;
#[macro_use]
extern crate g;
extern crate hyper;
extern crate libloading;
extern crate proto;
#[macro_use]
extern crate rental;
extern crate sha3;
extern crate sodiumoxide;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_timer;

mod basic;
#[macro_use]
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
use std::process;
use std::sync::mpsc;
use std::thread;

use futures::future::Future;
use futures::sync::mpsc::unbounded;

use g::gfx::Device;
use g::gfx_text;
use g::gfx_window_glutin;
use g::glutin::{self, GlContext};
use proto::{Bincoded, Bytes, handshake};

use common::OurFuture;
use errors::*;
use net::DriverUpdate;
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

    let controller = Controller::new();
    let control_tx = controller.control_tx.clone();
    let update_tx = controller.update_tx.clone();

    thread::Builder::new().name("net".into()).spawn(
        move || {
            net::thread(server_addr, move |sock| -> OurFuture<_> {
                let control_tx = control_tx.clone();
                let update_tx = update_tx.clone();
                let hello = handshake::Hello::Newbie;
                box common::write_bincoded(sock, &hello)
                    .and_then(|(sock, _)| receive::fetch_driver(sock))
                    .and_then(move |(sock, info, path)| {
                        println!("driver {}", info.digest.short_hex());

                        let (driver_tx, driver_rx) = mpsc::channel();
                        let (tx, rx) = unbounded();
                        let comms = connector::DriverComms::new(driver_rx, tx, control_tx);

                        // inform the draw thread about our new driver
                        update_tx.send((path, box comms))
                            .map(|()| (sock, net::ClientSide { tx: driver_tx, rx: rx }))
                            .map_err(|_| ErrorKind::BrokenComms.into())
                    })
            })
        }
    ).expect("net thread");

    let mut events_loop = g::EventsLoop::new();
    let window = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768);
    let context = glutin::ContextBuilder::new()
        .with_vsync(true);

    let (window, mut device, mut factory, main_color, main_depth) =
        gfx_window_glutin::init::<g::ColorFormat, g::DepthFormat>(window, context, &events_loop);

    let mut engine = Hot::new(controller, &mut factory, main_color, main_depth)?;

    let mut encoder = factory.create_command_buffer().into();

    let mut alive = true;
    loop {
        events_loop.poll_events(
            |event| {
                let event = match event {
                    g::Event::WindowEvent { event, .. } => event,
                    g::Event::DeviceEvent { .. } => return,
                    g::Event::Awakened => return,
                };
                if render_loop::should_quit(&event) {
                    alive = false;
                    return;
                }
                if let g::WindowEvent::Resized(_w, _h) = event {
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

        engine.update(&mut (), &mut factory)?;
        engine.draw(&mut encoder)?;

        encoder.flush(&mut device);
        window.swap_buffers().chain_err(|| "swapping buffers")?;
        device.cleanup();
    }

    drop(engine); // waits for driver cleanup
    Ok(())
}

/// Hub for control/upgrade messages. Owned by the engine.
pub struct Controller {
    control_rx: mpsc::Receiver<Bytes>,
    pub control_tx: mpsc::Sender<Bytes>,
    update_rx: mpsc::Receiver<Update>,
    pub update_tx: mpsc::Sender<Update>,
}

impl Controller {
    fn new() -> Self {
        // for receiving signals from the driver
        let (control_tx, control_rx) = mpsc::channel::<Bytes>();
        // for sending new drivers to the draw thread
        let (update_tx, update_rx) = mpsc::channel::<Update>();
        Controller { control_rx, control_tx, update_rx, update_tx }
    }
}

/// Our driver-loading Engine.
struct Hot {
    basic_vis: basic::Renderer<g::Res>,
    controller: Controller,
    driver: Option<(connector::Driver, g::GfxBox)>,
    main_color: g::RenderTargetView,
    main_depth: g::DepthStencilView,
    text: gfx_text::Renderer<g::Res, g::Factory>,
}

type Update = DriverUpdate<connector::DriverComms>;

impl Hot {
    fn new(
        controller: Controller,
        factory: &mut g::Factory,
        rtv: g::RenderTargetView,
        dsv: g::DepthStencilView,
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
                controller,
                driver: None,
                main_color: rtv,
                main_depth: dsv,
                text,
            }
        )
    }

    pub fn obey(&mut self, msg: handshake::UpControl) -> Result<()> {
        use handshake::UpControl::*;
        match msg {
            Download(uri, info) => println!("download {:?} {}", uri, info.digest.short_hex()),
        }
        Ok(())
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

    fn update(&mut self, _: &mut (), factory: &mut g::Factory) -> Result<()> {

        // xxx handle disconnected pipe
        if let Ok(bytes) = self.controller.control_rx.try_recv() {
            let coded = unsafe { Bincoded::from_bytes(bytes) };
            match coded.deserialize() {
                Ok(msg) => self.obey(msg)?,
                Err(e) => println!("control: de: {:?}", e),
            }
        }

        // xxx handle disconnected pipe
        if let Ok((path, comms)) = self.controller.update_rx.try_recv() {
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

        if let Some((ref mut driver, ref mut ctx)) = self.driver {
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
