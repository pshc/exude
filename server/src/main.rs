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
use std::path::{Path, PathBuf};
use std::process;
use std::rc::Rc;

use futures::future::{self, Future, IntoFuture, Loop};
use futures::stream::Stream;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::io::{ReadHalf, WriteHalf};

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
                let (r, w) = sock.split();
                let client = Rc::new(ClientShared {
                    addr,
                });
                let io = ClientIO { r, w, client };
                handle.spawn(serve_client(io));
                Ok(())
            }
        );

    core.run(server).chain_err(|| "core listener failed")
}

struct ClientIO {
    r: ReadHalf<TcpStream>,
    w: WriteHalf<TcpStream>,
    client: Rc<ClientShared>,
}

struct ClientShared {
    addr: SocketAddr,
}

fn serve_client(io: ClientIO) -> Box<Future<Item = (), Error = ()>> {
    let ClientIO { r, w, client } = io;
    println!("new client from {}", client.addr);

    let hello = common::read_bincoded(r);
    let client_rc2 = client.clone();

    box hello
            .and_then(
        move |(r, hello)| -> OurFuture<_> {
            use handshake::Hello::*;
            use handshake::Welcome::{Current, Obsolete};
            println!("{} is here: {:?}", client.addr, hello);
            let info = try_box!(read_latest_metadata());

            let write: OurFuture<_> = match hello {
                Cached(ref d) | Oneshot(ref d) if d == &info.digest => {
                    box common::write_bincoded(w, &Current).map(|(w, _)| w)
                }
                Newbie | Cached(_) => {
                    // send them the up-to-date driver
                    let driver = try_box!(HashedHeapFile::from_metadata(info));
                    driver.write_to(w)
                }
                Oneshot(_) => box common::write_bincoded(w, &Obsolete).map(|(w, _)| w),
            };

            box write.map(|w| ClientIO { r, w, client })
        }
    )
            .and_then(
        move |io| {

            future::loop_fn(
                io, move |io| {

                    let ClientIO { r, w, client } = io;
                    let read_req = common::read_bincoded(r);
                    let dispatch_req = read_req.and_then(
                        move |(r, req)| -> OurFuture<_> {
                            let io = ClientIO { r, w, client };
                            io.handle_request(req)
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
            let client = client_rc2;
            println!("{} error: {}", client.addr, err);
            for e in err.iter().skip(1) {
                println!("  caused by: {}", e);
            }
        }
    )
}

impl ClientIO {
    fn handle_request(self, req: api::UpRequest) -> OurFuture<future::Loop<(), Self>> {
        use api::UpRequest::*;
        let ClientIO { r, w, client } = self;

        match req {
            Ping(n) => {
                println!("{} pinged ({})", client.addr, n);

                box common::write_bincoded(w, &api::DownResponse::Pong(n))
                        .and_then(move |(w, _)| Ok(Loop::Continue(ClientIO { r, w, client })))
            }
            Bye => {
                println!("{} says bye", client.addr);
                box future::ok(Loop::Break(()))
            }
        }
    }
}

fn read_latest_metadata() -> Result<DriverInfo> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push("latest.meta");
    read_metadata(&path)
}

fn read_metadata(path: &Path) -> Result<DriverInfo> {
    let mut vec = Vec::new();
    let n = File::open(path)
        .and_then(|mut f| f.read_to_end(&mut vec))
        .chain_err(|| format!("couldn't open metadata ({})", path.display()))?;
    if n == 0 {
        bail!("metadata was empty");
    }
    let info: DriverInfo =
        unsafe { Bincoded::from_vec(vec) }
            .deserialize()
            .chain_err(|| format!("couldn't decode metadata ({})", path.display()))?;
    Ok(info)
}

/// The bytes and hash digest of a file stored on the heap.
#[derive(Debug)]
struct HashedHeapFile(Vec<u8>, DriverInfo);

impl HashedHeapFile {
    /// Read the signed driver into memory.
    fn from_metadata(info: DriverInfo) -> Result<Self> {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.pop();
        // presumably we would look up by digest into the repo here
        let ref bin_path = root.join("latest.bin");

        let len = info.len;
        assert!(len <= handshake::INLINE_MAX);
        let mut driver_buf = Vec::with_capacity(len);
        let eof = File::open(bin_path)
            .and_then(
                |mut bin| {
                    unsafe {
                        driver_buf.set_len(len);
                    }
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

        box common::write_bincoded(w, &resp)
                .and_then(
            move |(w, _)| {
                tokio_io::io::write_all(w, buf)
                    .then(|res| res.chain_err(|| "couldn't write inline driver"))
            }
        )
                .map(|(w, _)| w)
    }
}
