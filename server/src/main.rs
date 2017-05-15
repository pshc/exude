#![feature(box_syntax)]
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;

mod common;

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;

use futures::{Future, IntoFuture, Stream, future};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};

use common::OurFuture;
use proto::{Bincoded, DriverInfo, api, handshake};

mod errors {
    use proto;
    error_chain! {
        errors { AlreadyRunning }
        foreign_links {
            Bincode(proto::bincoded::Error);
        }
    }
}
use errors::*;

fn main() {
    let addr: SocketAddr = ([127, 0, 0, 1], 2001).into();
    let oops = "couldn't write to stderr";

    match serve(&addr) {
        Ok(()) => (),
        Err(Error(ErrorKind::AlreadyRunning, _)) => {
            writeln!(io::stderr(), "Server already listening.").expect(oops);
            process::exit(2);
        }
        Err(e) => {
            let stderr = io::stderr();
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
}

/// hopefully replace with `?` later
macro_rules! try_box {
    ($expr:expr) => (match $expr {
        Ok(val) => val,
        Err(err) => {
            return Box::new(Err(From::from(err)).into_future())
        }
    })
}

fn serve(addr: &SocketAddr) -> Result<()> {
    let mut core = Core::new().chain_err(|| "tokio/mio pls")?;
    let handle = core.handle();

    // attempt to be a server
    let listener = match TcpListener::bind(&addr, &handle) {
        Ok(listener) => listener,
        Err(ref e) if e.kind() == io::ErrorKind::AddrInUse => bail!(ErrorKind::AlreadyRunning),
        Err(e) => Err(e).chain_err(|| format!("couldn't bind {}", addr))?,
    };
    println!("Listening on: {}", addr);

    let server = listener
        .incoming()
        .for_each(
            |(sock, addr)| {
                handle.spawn(serve_client(sock, addr));
                Ok(())
            }
        );

    core.run(server).chain_err(|| "core listener failed")
}

fn serve_client(sock: TcpStream, addr: SocketAddr) -> Box<Future<Item = (), Error = ()>> {
    use futures::future::{Loop, loop_fn};

    println!("new client from {}", addr);

    let (r, w) = sock.split();
    let hello = common::read_bincoded(r);

    box hello
            .and_then(
        move |(r, hello)| -> OurFuture<_> {
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
        }
    )
            .and_then(
        move |rw| {

            loop_fn(
                rw, move |(r, w)| {

                    let read_req = common::read_bincoded(r);
                    let dispatch_req = read_req.and_then(
                        move |(r, req)| -> OurFuture<_> {
                            match req {
                                api::UpRequest::Ping(n) => {
                                    println!("{} pinged ({})", addr, n);

                                    box common::write_bincoded(w, &api::DownResponse::Pong(n))
                                            .and_then(move |(w, _)| Ok(Loop::Continue((r, w))))
                                }
                                api::UpRequest::Bye => {
                                    println!("{} says bye", addr);
                                    box future::ok(Loop::Break(()))
                                }
                            }
                        }
                    );

                    dispatch_req
                }
            )

        }
    )
            .map(|_r| ())
            .map_err(
        move |err| {
            println!("{} error: {}", addr, err);
            for e in err.iter().skip(1) {
                println!("  caused by: {}", e);
            }
        }
    )
}

/// The bytes and hash digest of a file stored on the heap.
#[derive(Debug)]
struct HashedHeapFile(Vec<u8>, DriverInfo);

impl HashedHeapFile {
    /// Read the latest signed driver into memory.
    fn read_latest() -> Result<Self> {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.pop();
        let ref bin_path = root.join("latest.bin");
        let ref meta_path = root.join("latest.meta");

        let mut meta_vec = Vec::new();
        let n = File::open(meta_path)
            .and_then(|mut f| f.read_to_end(&mut meta_vec))
            .chain_err(|| format!("couldn't open metadata ({})", meta_path.display()))?;
        if n == 0 {
            bail!("metadata was empty");
        }
        let info: DriverInfo =
            unsafe { Bincoded::from_vec(meta_vec) }
                .deserialize()
                .chain_err(|| format!("couldn't decode metadata ({})", meta_path.display()))?;
        let len = info.len;

        assert!(info.len <= handshake::INLINE_MAX);
        let mut driver_buf = Vec::with_capacity(len);
        unsafe {
            driver_buf.set_len(len);
        }
        let eof = File::open(bin_path)
            .and_then(
                |mut bin| {
                    bin.read_exact(&mut driver_buf)?;
                    Ok(bin.read(&mut [0])? == 0)
                }
            )
            .chain_err(|| format!("couldn't open driver ({})", bin_path.display()))?;
        if !eof {
            bail!("driver ({}) is wrong length", bin_path.display());
        }

        // xxx we may want to re-verify hash or sig here?
        // although the client will check them anyway

        Ok(HashedHeapFile(driver_buf, info))
    }

    /// Write an InlineDriver header and then the bytes.
    fn write_to<W: AsyncWrite + 'static>(self, w: W) -> OurFuture<W> {
        let HashedHeapFile(buf, info) = self;
        assert!(buf.len() < handshake::INLINE_MAX);
        let resp = handshake::Welcome::InlineDriver(info);
        let coded = match Bincoded::new(&resp) {
            Ok(b) => b,
            Err(e) => return box future::err(e.into()),
        };

        box common::write_with_length(w, coded)
                .and_then(
            move |(w, _)| {
                tokio_io::io::write_all(w, buf)
                    .then(|res| res.chain_err(|| "couldn't write inline driver"))
            }
        )
                .map(|(w, _)| w)
    }
}
