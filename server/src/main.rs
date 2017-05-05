#![feature(box_syntax)]

extern crate futures;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;

mod common;

use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::net::SocketAddr;
use std::path::PathBuf;

use futures::{Future, IntoFuture, Stream};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};

use common::IoFuture;
use proto::{Bincoded, DriverInfo, api, handshake};

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();

    if let Err(e) = serve(&addr) {
        if e.kind() == ErrorKind::AddrInUse {
            println!("Server already listening.");
        } else {
            panic!(e);
        }
    }
}

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

    let server = listener
        .incoming()
        .for_each(
            |(sock, addr)| {
                handle.spawn(serve_client(sock, addr));
                Ok(())
            },
        );

    core.run(server)
}

fn serve_client(sock: TcpStream, addr: SocketAddr) -> Box<Future<Item = (), Error = ()>> {
    use futures::future::{Loop, loop_fn};

    println!("new client from {}", addr);

    let (r, w) = sock.split();
    let hello = common::read_bincoded(r);

    box hello
            .and_then(
        move |(r, hello)| -> IoFuture<_> {
            println!("{} is here: {:?}", addr, hello);

            match hello {
                handshake::Hello(None) => {
                    // send them the up-to-date driver
                    let driver = try_box!(HashedHeapFile::read_latest());
                    box driver.write_to(w).and_then(move |w| Ok((r, w)))
                }
                handshake::Hello(Some(_digest)) => {
                    // check if digest is up-to-date; if not, send delta
                    unimplemented!()
                }
            }
        },
    )
            .and_then(
        move |rw| {

            loop_fn(
                rw, move |(r, w)| {

                    let read_req = common::read_bincoded(r);
                    let dispatch_req = read_req.and_then(
                        move |(r, req)| -> IoFuture<_> {
                            match req {
                                api::UpRequest::Ping(n) => {
                                    println!("{} pinged ({})", addr, n);

                                    box common::write_bincoded(w, &api::DownResponse::Pong(n))
                                            .and_then(move |(w, _)| Ok(Loop::Continue((r, w))))
                                }
                                api::UpRequest::Bye => {
                                    println!("{} says bye", addr);
                                    box futures::future::ok(Loop::Break(()))
                                }
                            }
                        },
                    );

                    dispatch_req
                }
            )

        },
    )
            .map(|_r| ())
            .map_err(
        move |err| {
            println!("{} error: {}", addr, err);
        },
    )
}

/// The bytes and hash digest of a file stored on the heap.
#[derive(Debug)]
struct HashedHeapFile(Vec<u8>, DriverInfo);

impl HashedHeapFile {
    /// Read the latest signed driver into memory.
    fn read_latest() -> io::Result<Self> {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.pop();
        let bin_path = root.join("latest.bin");
        let meta_path = root.join("latest.meta");

        let mut meta_vec = Vec::new();
        assert!(File::open(meta_path)?.read_to_end(&mut meta_vec)? > 0);
        let info: DriverInfo = unsafe { Bincoded::from_vec(meta_vec) }.deserialize()?;
        let len = info.len;

        assert!(info.len <= handshake::INLINE_MAX);
        let mut driver_buf = Vec::with_capacity(len);
        unsafe {
            driver_buf.set_len(len);
        }
        let mut bin = File::open(bin_path)?;
        bin.read_exact(&mut driver_buf)?;
        if bin.read(&mut [0])? > 0 {
            return Err(io::Error::new(ErrorKind::InvalidData, "latest bin is wrong length"),);
        }
        drop(bin);

        // xxx we may want to re-verify hash or sig here?
        // although the client will check them anyway

        Ok(HashedHeapFile(driver_buf, info))
    }

    /// Write an InlineDriver header and then the bytes.
    fn write_to<W: AsyncWrite + 'static>(self, w: W) -> IoFuture<W> {
        let HashedHeapFile(buf, info) = self;
        assert!(buf.len() < handshake::INLINE_MAX);
        let resp = handshake::Welcome::InlineDriver(info);
        let coded = Bincoded::new(&resp);

        box futures::future::lazy(|| coded)
                .and_then(move |coded| common::write_with_length(w, coded))
                .and_then(move |(w, _)| tokio_io::io::write_all(w, buf))
                .and_then(move |(w, _)| Ok(w))
    }
}
