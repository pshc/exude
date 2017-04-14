#![feature(alloc_system, box_syntax, conservative_impl_trait, drop_types_in_const)]

// hack: use alloc_system so the client & driver always share the same allocator...
//       would be nice to share jemalloc somehow
extern crate alloc_system;
extern crate bincode;
extern crate digest;
extern crate futures;
extern crate gfx;
extern crate gfx_window_glutin;
extern crate glutin;
extern crate libloading;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate sha3;
extern crate tokio_core;
extern crate tokio_io;

mod common;
mod env;
mod receive;

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write, stdout};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use futures::{IntoFuture, Future, Stream};
use futures::future::Either;
use gfx::Device;
use libloading::{Library, Symbol};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};

use env::bincoded::Bincoded;
use env::DriverEnv;


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
    let mut core = Core::new().unwrap();
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
                    common::UpRequest::Ping(n) => {
                        println!("{} pinged ({})", addr, n);
                        // pong?
                        Ok(Loop::Continue((r, w)))
                    }
                    common::UpRequest::Bye => {
                        println!("{} says bye", addr);
                        Ok(Loop::Break(()))
                    }
                }
            });

            // join asynchronous writes? how do we share the writer?

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
        let dylib = concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/libdriver.dylib");
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

pub struct Api<'lib> {
    s_driver: Symbol<'lib, extern fn(Box<DriverEnv>)>,
    s_version: Symbol<'lib, extern fn() -> u32>,
}

impl<'lib> Api<'lib> {
    pub unsafe fn new(lib: &'lib Library) -> libloading::Result<Self> {
        Ok(Api {
            s_driver: lib.get(b"driver\0")?,
            s_version: lib.get(b"version\0")?,
        })
    }

    pub fn driver(&self, env: Box<DriverEnv>) {
        (*self.s_driver)(env)
    }

    pub fn version(&self) -> u32 {
        (*self.s_version)()
    }
}

fn load(path: &Path, env: Box<DriverEnv>) -> libloading::Result<()> {
    let lib = Library::new(path)?;
    let api = unsafe { Api::new(&lib)? };

    print!("loaded driver ");
    stdout().flush().ok().expect("flush1");
    println!("v{}", api.version());
    stdout().flush().ok().expect("flush2");

    api.driver(env);

    return Ok(())
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
    let (update_tx, update_rx) = mpsc::channel::<(PathBuf, Box<DriverEnv>)>();

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
                let (tx, rx) = mpsc::channel();
                let env = DriverEnv {rx:driver_rx, tx:tx};

                // inform the draw thread about our new driver
                update_tx.send((path, box env)).unwrap();

                // now be a dumb pipe, but with length-delimited messages for some reason??
                // todo: read more than one message
                let read = common::read_with_length(r).and_then(move |(_r, vec)| {
                    driver_tx.send(Some(vec)).map_err(|e| {
                        // note: SendError holds the Option<Vec<u8>>... is that what we want?
                        io::Error::new(ErrorKind::BrokenPipe, e)
                    })
                });

                let (relay_tx, relay_rx) = futures::sync::mpsc::unbounded();

                let write = relay_rx
                .map_err(|()| io::Error::new(ErrorKind::Other, "write relay broke?!"))
                // TEMP: explicit type and box to avoid ICE
                .fold(w, |w, msg|
                    -> Box<Future<Item=tokio_io::io::WriteHalf<TcpStream>, Error=io::Error>>
                {
                    box common::write_with_length(w, msg).map(|(w, _)| w)
                })
                .map(|_| println!("write: donezo"));

                // HACK -- wow this is a stupid extra hop
                // just give the driver a callback or something, smh
                let _relay_thread = thread::spawn(move || {
                    loop {
                        match rx.recv() {
                            Ok(None) => break,
                            Ok(Some(msg)) => {
                                // NOPE
                                use futures::sync::mpsc::UnboundedSender;
                                if let Err(e) = UnboundedSender::send(&relay_tx, msg) {
                                    println!("write relay: {:?}", e);
                                    break
                                }
                            }
                            Err(mpsc::RecvError) => {
                                println!("write relay: pipe broken");
                                break
                            }
                        }
                    }
                    println!("read: donezo");
                });

                read.join(write)
            })
        });

        core.run(client).unwrap();
    });

    // otherwise, we're a client
    pub type ColorFormat = gfx::format::Rgba8;
    pub type DepthFormat = gfx::format::Depth; //Stencil;
    const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.1, 1.0];
    let builder = glutin::WindowBuilder::new()
        .with_title("Germ".to_string())
        .with_dimensions(1024, 768)
        .with_vsync();
    let (window, mut device, mut factory, mut main_color, mut main_depth) =
        gfx_window_glutin::init::<ColorFormat, DepthFormat>(builder);

    let mut encoder: gfx::Encoder<_, _> = factory.create_command_buffer().into();

    'main: loop {
        if let Ok((path, env)) = update_rx.try_recv() {
            println!("render: updating driver...");
            io::stdout().flush().unwrap();
            load(&path, env).unwrap();
            println!("render: driver updated!");
            io::stdout().flush().unwrap();
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

        encoder.clear(&main_color, CLEAR_COLOR);

        //encoder.draw(...);
        std::thread::sleep(std::time::Duration::from_millis(10));

        encoder.flush(&mut device);
        window.swap_buffers().unwrap();
        device.cleanup();
    }
}
