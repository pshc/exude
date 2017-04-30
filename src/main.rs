#![feature(box_syntax, conservative_impl_trait, drop_types_in_const)]

extern crate bincode;
extern crate digest;
extern crate futures;
extern crate g;
extern crate libc;
extern crate libloading;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate sha3;
extern crate tokio_core;
extern crate tokio_io;

mod common;
mod connector;
#[path="../driver/src/env.rs"]
mod env;
mod receive;

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use futures::{IntoFuture, Future, Stream};
use futures::future::Either;
use gfx::Device;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};

use g::gfx;
use g::gfx_device_gl;
use g::gfx_window_glutin;
use g::glutin;

use env::bincoded::Bincoded;


/// hopefully replace with `?` later
macro_rules! try_box {
    ($expr:expr) => (match $expr {
        Result::Ok(val) => val,
        Result::Err(err) => {
            // todo call `into_future` unambiguously
            return Box::new(Result::Err(From::from(err)).into_future())
        }
    })
}

fn serve(addr: &SocketAddr) -> io::Result<()> {
    let mut core = Core::new()?;
    let handle = core.handle();

    // attempt to be a server
    let listener = TcpListener::bind(&addr, &handle)?;
    println!("listening to {:?}", addr);

    let server = listener.incoming().for_each(|(sock, addr)| {
        handle.spawn(serve_client(sock, addr));
        Ok(())
    });

    core.run(server)
}

fn serve_client(sock: TcpStream, addr: SocketAddr) -> Box<Future<Item=(), Error=()>> {
    use futures::future::{loop_fn, Loop};

    println!("new client from {}", addr);

    let (r, w) = sock.split();
    let hello = common::read_bincoded(r);

    box hello.and_then(move |(r, hello)| -> Box<Future<Item=_, Error=io::Error>> {
        println!("{} is here: {:?}", addr, hello);

        match hello {
            common::Hello(None) => {
                // send them the up-to-date driver
                let driver = try_box!(HashedHeapFile::from_debug_dylib());
                box driver.write_to(w).and_then(move |w| {
                    Ok((r, w))
                })
            }
            common::Hello(Some(_digest)) => {
                // check if digest is up-to-date; if not, send delta
                unimplemented!()
            }
        }
    })
    .and_then(move |rw| {

        loop_fn(rw, move |(r, w)| {

            let read_req = common::read_bincoded(r);
            let dispatch_req = read_req.and_then(move |(r, req)| {
                match req {
                    env::UpRequest::Ping(n) => {
                        println!("{} pinged ({})", addr, n);

                        Either::A(common::write_bincoded(w, &env::DownResponse::Pong(n))
                            .and_then(move |(w, _)| Ok(Loop::Continue((r, w)))))
                    }
                    env::UpRequest::Bye => {
                        println!("{} says bye", addr);
                        Either::B(Ok(Loop::Break(())).into_future())
                    }
                }
            });

            dispatch_req
        })

    }).map(|_r| ()).map_err(move |err| {
        println!("{} error: {}", addr, err);
    })
}

/// The bytes and hash digest of a file stored on the heap.
#[derive(Debug)]
struct HashedHeapFile(Vec<u8>, common::Digest);

impl HashedHeapFile {
    /// Read the currently compiled debug driver into memory.
    fn from_debug_dylib() -> io::Result<Self> {
        let dylib = concat!(env!("CARGO_MANIFEST_DIR"), "/driver/target/debug/libdriver.dylib");
        let file_len = fs::metadata(dylib)?.len();
        assert!(file_len <= receive::INLINE_MAX as u64);
        let len = file_len as usize;
        let mut file = File::open(dylib)?;
        let mut driver_buf = Vec::with_capacity(len);
        unsafe {
            driver_buf.set_len(len);
        }
        file.read_exact(&mut driver_buf)?;

        // xxx don't rehash every time durr
        let digest = receive::utils::digest_from_bytes(&driver_buf[..]);

        Ok(HashedHeapFile(driver_buf, digest))
    }

    /// Write an InlineDriver header and then the bytes.
    fn write_to<W: AsyncWrite>(self, w: W) -> impl Future<Item=W, Error=io::Error> {
        let HashedHeapFile(buf, digest) = self;
        assert!(buf.len() < receive::INLINE_MAX);
        let len = buf.len() as u32;
        let resp = common::Welcome::InlineDriver(len, digest);
        let coded = Bincoded::new(&resp);

        futures::future::lazy(|| coded)
            .and_then(move |coded| common::write_with_length(w, coded))
            .and_then(move |(w, _)| tokio_io::io::write_all(w, buf))
            .and_then(move |(w, _)| Ok(w))
    }
}

/// Downloads the newest driver (if needed), returning its path.
fn fetch_driver<R: AsyncRead + 'static>(reader: R)
    -> Box<Future<Item=(R, common::Digest, PathBuf), Error=io::Error>>
{
    box common::read_bincoded(reader)
      .and_then(|(reader, welcome)| {

        match welcome {
            common::Welcome::Current => unimplemented!(),
            common::Welcome::InlineDriver(len, digest) => {
                println!("receiving driver {} ({}kb)", digest.short_hex(), len/1000);

                let download = receive::verify_and_save(len as usize, digest, reader)
                    .and_then(Ok);

                Either::A(download)
            }
            common::Welcome::DownloadDriver(url, digest) => {
                println!("TODO download {} and check {}", url, digest);
                let download = io::Error::new(ErrorKind::Other, "todo download");
                Either::B(Err(download).into_future())
            }
        }
    })
}

fn main() {
    let addr = "127.0.0.1:2001".parse().unwrap();

    match serve(&addr) {
        Ok(()) => return,
        Err(ref e) if e.kind() == ErrorKind::AddrInUse => (),
        Err(e) => panic!(e),
    }

    // for sending a new driver from net to draw thread
    let (update_tx, update_rx) = mpsc::channel::<(PathBuf, Box<connector::DriverComms>)>();

    let _net_thread = thread::spawn(move || {
        let mut core = Core::new().unwrap();
        let handle = core.handle();

        let client = TcpStream::connect(&addr, &handle).and_then(|sock| {
            let (reader, writer) = sock.split();

            let greeting = {
                let cached_driver = None;
                let hello = common::Hello(cached_driver);
                common::write_bincoded(writer, &hello).and_then(|(w, _)| Ok(w))
            };

            let welcome = fetch_driver(reader);

            welcome.join(greeting).and_then(move |((r, digest, path), w)| {
                println!("driver {}", digest.short_hex());

                let (driver_tx, driver_rx) = mpsc::channel();
                let (tx, rx) = futures::sync::mpsc::unbounded();
                let comms = connector::DriverComms {rx: driver_rx, tx: tx};

                // inform the draw thread about our new driver
                update_tx.send((path, box comms)).unwrap();

                // now be a dumb pipe, but with length-delimited messages for some reason??
                // todo: read more than one message
                let read = common::read_with_length(r).and_then(move |(_r, vec)| {
                    driver_tx.send(vec.into_boxed_slice()).map_err(|_| {
                        io::Error::new(ErrorKind::BrokenPipe, "core: done reading")
                    })
                });

                let write = rx
                .map_err(|()| io::Error::new(ErrorKind::BrokenPipe, "core: done writing"))
                // TEMP: explicit type and box to avoid ICE
                .fold(w, |w, msg|
                    -> Box<Future<Item=tokio_io::io::WriteHalf<TcpStream>, Error=io::Error>>
                {
                    box common::write_with_length(w, msg).map(|(w, _)| w)
                })
                .map(|_| println!("write: donezo"));

                read.join(write)
            })
        });

        core.run(client).unwrap();
    });

    // otherwise, we're a client
    type DepthFormat = gfx::format::Depth;
    const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.1, 1.0];
    const READY_COLOR: [f32; 4] = [0.0, 0.5, 0.0, 1.0];

    let builder = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, mut main_color, mut main_depth) =
        gfx_window_glutin::init::<env::ColorFormat, DepthFormat>(builder);

    let mut encoder: gfx::Encoder<_, _> = factory.create_command_buffer().into();

    let mut driver = None;

    'main: loop {
        if let Ok((path, comms)) = update_rx.try_recv() {

            println!("loading driver...");
            io::stdout().flush().unwrap();
            let loaded = connector::load(&path, comms).unwrap();
            println!("driver setup!");
            io::stdout().flush().unwrap();

            let ctx = (loaded.gl_setup())(&mut factory, box main_color.clone()).unwrap();
            driver = Some((loaded, ctx));
        }

        for event in window.poll_events() {
            use glutin::VirtualKeyCode::*;

            match event {
                glutin::Event::KeyboardInput(_, _, Some(Escape)) |
                glutin::Event::KeyboardInput(_, _, Some(Grave)) |
                glutin::Event::Closed => break 'main,

                glutin::Event::Resized(_w, _h) => {
                    gfx_window_glutin::update_views(&window,
                        &mut main_color, &mut main_depth);
                },
                _ => {},
            }
            // we should probably forward the events to driver?
        }

        encoder.clear(&main_color, if driver.is_some() { READY_COLOR } else { CLEAR_COLOR });

        if let Some((ref driver, ref ctx)) = driver {
            driver.gl_draw()(&**ctx, &mut encoder);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));

        encoder.flush(&mut device);
        window.swap_buffers().unwrap();
        device.cleanup();
    }

    if let Some((driver, ctx)) = driver {
        driver.gl_cleanup()(ctx);
    }
}
