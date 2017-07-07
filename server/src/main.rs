#![feature(box_patterns, box_syntax)]
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;
use std::rc::Rc;

use futures::future::{self, Future, IntoFuture};
use futures::stream::{self, Stream};
use futures::unsync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::io::{ReadHalf, WriteHalf};

use common::OurFuture;
use proto::{Bincoded, Bytes, BytesMut, DriverInfo, api, handshake};
use proto::serde::Serialize;

mod errors {
    use proto;
    error_chain! {
        errors { AlreadyRunning GracefulDisconnect }
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
    // preload the latest driver (if any)
    let current_driver = HashedHeapFile::latest();

    let mut core = Core::new().chain_err(|| "tokio/mio pls")?;
    let handle = core.handle();

    // attempt to be a server
    let listener = match TcpListener::bind(&addr, &handle) {
        Ok(listener) => listener,
        Err(ref e) if e.kind() == io::ErrorKind::AddrInUse => bail!(ErrorKind::AlreadyRunning),
        Err(e) => Err(e).chain_err(|| format!("couldn't bind {}", addr))?,
    };
    println!("Listening on: {}", addr);

    let god = Rc::new(RefCell::new(God::new(current_driver.clone())));

    let server = listener
        .incoming()
        .for_each(
            |(sock, addr)| {
                let (outbox_tx, outbox_rx) = unbounded();

                let entry = Rc::new(ClientEntry { addr, outbox_tx });
                let id = god.borrow_mut().add_client(entry.clone());

                let (r, w) = sock.split();
                let io = ClientIO {
                    id, r, w,
                    client: entry,
                    upstream: god.clone(),
                    outbox_rx,
                };
                handle.spawn(serve_client(io));
                Ok(())
            }
        );

    // listen for upgrades
    let (upgrade_tx, upgrade_rx) = unbounded();
    serve_controller(core.handle(), upgrade_tx);

    // broadcast upgrades to clients
    let god = god.clone();
    core.handle().spawn(upgrade_rx.for_each(move |info| {
        match Bincoded::new(&info) {
            Ok(bincoded) => {
                let driver = HashedHeapFile::from_metadata(info)
                    .map_err(|e| writeln!(io::stderr(), "load driver: {}", e).expect("stderr"))?;

                // the update seems OK, so save it for future clients
                *current_driver.borrow_mut() = Some(Rc::new(driver));

                let bytes = bincoded.into();
                let n = god.borrow_mut().broadcast(bytes);
                if n > 0 {
                    println!("Sent update to {} client(s)", n);
                }
                Ok(())
            }
            Err(e) => {
                writeln!(io::stderr(), "bincode driver: {}", e).expect("stderr");
                Err(())
            }
        }
    }));

    core.run(server).chain_err(|| "core listener failed")
}

fn serve_controller(handle: tokio_core::reactor::Handle, tx: UnboundedSender<DriverInfo>) {
    let addr: SocketAddr = ([127, 0, 0, 1], 2002).into();
    let listener = TcpListener::bind(&addr, &handle).expect("couldn't bind controller");
    println!("Controller listening on: {}", addr);

    fn relay_upgrade(
        sock: TcpStream,
        tx: UnboundedSender<DriverInfo>)
        -> Box<Future<Item = (), Error = ()>> {

        let (r, _) = sock.split();
        box common::read_bincoded::<_, DriverInfo>(r).and_then(move |(_, info)| {
            tx.send(info).chain_err(|| "couldn't send upgrade")
        })
            .map_err(|e| println!("control: {:?}", e))
    }

    let handle2 = handle.clone();
    let controller = listener
        .incoming()
        .for_each(
            move |(sock, _)| {
                handle2.spawn(relay_upgrade(sock, tx.clone()));
                Ok(())
            }
        )
        .map_err(|e| println!("control: {:?}", e));

    handle.spawn(controller);
}

type ClientId = u32;
type CurrentDriver = Rc<RefCell<Option<Rc<HashedHeapFile>>>>;

/// Overall server state.
/// Try to not let this become a bottleneck.
struct God {
    ctr: ClientId,
    clients: BTreeMap<ClientId, Rc<ClientEntry>>,

    current: CurrentDriver,
}

impl God {
    fn new(current: CurrentDriver) -> Self {
        God {
            ctr: 0,
            clients: BTreeMap::new(),
            current,
        }
    }
}

trait Upstream {
    fn add_client(&mut self, client: Rc<ClientEntry>) -> ClientId;
    fn remove_client(&mut self, id: ClientId);
    /// Returns the number of clients written to.
    fn broadcast(&mut self, bytes: Bytes) -> usize;

    fn current_driver(&self) -> Option<Rc<HashedHeapFile>>;
}

impl Upstream for God {
    fn add_client(&mut self, client: Rc<ClientEntry>) -> ClientId {
        self.ctr += 1;
        let id = self.ctr;
        let existing = self.clients.insert(id, client);
        assert!(existing.is_none());
        id
    }

    fn remove_client(&mut self, id: ClientId) {
        if self.clients.remove(&id).is_none() {
            writeln!(io::stderr(), "remove_client: #{} not present!", id).expect("stderr");
            debug_assert!(false);
        }
    }

    fn broadcast(&mut self, bytes: Bytes) -> usize {
        let mut n = 0;
        let mut dead_clients = vec![];
        for (id, client) in self.clients.iter() {
            if let Err(_) = client.outbox_tx.send(bytes.clone()) {
                dead_clients.push(*id);
            } else {
                n += 1;
            }
        }
        if !dead_clients.is_empty() {
            writeln!(io::stderr(), "clients already gone: {:?}", dead_clients).expect("stderr");
            for id in dead_clients {
                self.remove_client(id);
            }
        }
        n
    }

    fn current_driver(&self) -> Option<Rc<HashedHeapFile>> {
        self.current.borrow().clone()
    }
}

/// Stored in the table of clients.
struct ClientEntry {
    addr: SocketAddr,
    outbox_tx: UnboundedSender<Bytes>,
}

/// Bulk parameters for `serve_client`.
struct ClientIO<U: Upstream> {
    id: ClientId,
    r: ReadHalf<TcpStream>,
    w: WriteHalf<TcpStream>,
    client: Rc<ClientEntry>,
    upstream: Rc<RefCell<U>>,
    outbox_rx: UnboundedReceiver<Bytes>,
}

fn serve_client<U: Upstream + 'static>(io: ClientIO<U>) -> Box<Future<Item = (), Error = ()>> {

    let ClientIO { id, r, w, client, upstream, outbox_rx } = io;
    let remove_myself = {
        let up = upstream.clone();
        move || up.borrow_mut().remove_client(id)
    };

    let addr = client.addr;
    println!("new client #{} from {}", id, addr);

    let hello = common::read_bincoded(r);

    box hello
            .and_then(
        move |(r, hello)| -> OurFuture<_> {
            use handshake::Hello::*;
            use handshake::Welcome::{Current, Obsolete};
            println!("client #{} is {:?}", id, hello);
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
                Oneshot(digest) => {
                    box common::write_bincoded(w, &Obsolete).and_then(
                        move |_| {
                            bail!("{} has an obsolete oneshot: {}", addr, digest)
                        }
                    )
                }
            };

            box write.map(|w| (r, w))
        }
    )
            .and_then(
        move |(r, w)| {

            fn swap<A, B>((a, b): (A, B)) -> (B, A) {
                (b, a)
            }

            let requests = stream::unfold(r, |r| Some(common::read_bincoded(r).map(swap)))
                .for_each(move |req| client.handle_request(req, upstream.clone()));

            let writes = outbox_rx
                .map_err(|()| "UnboundedReceiver error".into())
                .fold(w, |w, msg| common::write_with_length(w, msg).map(|(w, _)| w));

            requests.join(writes).map(|_| ())
        }
    )
            .then(
        move |result| {
            remove_myself();
            if let Err(err) = result {
                println!("client #{} error: {}", id, err);
                for e in err.iter().skip(1) {
                    println!("  caused by: {}", e);
                }
            } else {
                println!("client #{} left", id);
            }
            Ok(())
        }
    )
}

impl ClientEntry {
    fn handle_request<U: Upstream>(
        &self,
        req: api::UpRequest,
        upstream: Rc<RefCell<U>>,
        ) -> OurFuture<()> {

        use api::UpRequest::*;

        match req {
            AcceptUpgrade(box client_sent_info) => {
                let current = match upstream.borrow().current_driver() {
                    Some(current) => current,
                    None => {
                        let msg = "no driver currently available";
                        return box future::failed(msg.into());
                    }
                };
                if current.info != client_sent_info {
                    let msg = "client sent invalid/outdated upgrade response";
                    return box future::failed(msg.into());
                }
                // just write inline for now...
                let bytes = current.bytes.clone();
                box self.outbox_tx.send(bytes).chain_err(|| "outbox_tx closed").into_future()
            }
            Ping(n) => {
                println!("{} pinged ({})", self.addr, n);
                box self.send(&api::DownResponse::Pong(n)).into_future()
            }
            Bye => {
                println!("{} says bye", self.addr);
                box future::failed(ErrorKind::GracefulDisconnect.into())
            }
        }
    }

    fn send<T: Serialize>(&self, msg: &T) -> Result<()> {
        let coded = Bincoded::new(msg)?;
        let bytes = coded.into();
        self.outbox_tx
            .send(bytes)
            .chain_err(|| "outbox_tx closed")
    }
}

fn read_latest_metadata() -> Result<DriverInfo> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push("latest.meta");
    read_metadata(&path)
}

fn read_metadata(path: &Path) -> Result<DriverInfo> {
    let len = fs::metadata(path)
        .chain_err(|| format!("couldn't stat metadata ({})", path.display()))?
        .len() as usize; // unchecked cast
    if len == 0 {
        bail!("metadata ({}) is empty", path.display());
    }
    let mut bytes = BytesMut::with_capacity(len);
    unsafe {
        bytes.set_len(len);
    }
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut bytes))
        .chain_err(|| format!("couldn't open metadata ({})", path.display()))?;
    // omitted: eof check
    let info: DriverInfo =
        unsafe { Bincoded::from_bytes(bytes.freeze()) }
            .deserialize()
            .chain_err(|| format!("couldn't decode metadata ({})", path.display()))?;
    Ok(info)
}

/// The bytes and hash digest of a file stored on the heap.
#[derive(Debug)]
struct HashedHeapFile { bytes: Bytes, info: DriverInfo }

impl HashedHeapFile {
    /// Read the signed driver into memory.
    fn from_metadata(info: DriverInfo) -> Result<Self> {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.pop();
        // presumably we would look up by digest into the repo here
        let ref bin_path = root.join("latest.bin");

        let len = info.len;
        assert!(len <= handshake::INLINE_MAX);
        let mut bytes = BytesMut::with_capacity(len);
        let eof = File::open(bin_path)
            .and_then(
                |mut bin| {
                    unsafe {
                        bytes.set_len(len);
                    }
                    bin.read_exact(&mut bytes)?;
                    Ok(bin.read(&mut [0])? == 0)
                }
            )
            .chain_err(|| format!("couldn't open driver ({})", bin_path.display()))?;
        if !eof {
            bail!("driver ({}) is wrong length", bin_path.display());
        }

        let bytes = bytes.freeze();

        // xxx we may want to re-verify hash or sig here?
        // although the client will check them anyway

        Ok(HashedHeapFile { bytes, info })
    }

    /// Write an InlineDriver header and then the bytes.
    fn write_to<W: AsyncWrite + 'static>(self, w: W) -> OurFuture<W> {
        let HashedHeapFile { bytes, info } = self;
        assert!(bytes.len() < handshake::INLINE_MAX);
        let resp = handshake::Welcome::InlineDriver(info);

        box common::write_bincoded(w, &resp)
                .and_then(
            move |(w, _)| {
                tokio_io::io::write_all(w, bytes)
                    .then(|res| res.chain_err(|| "couldn't write inline driver"))
            }
        )
                .map(|(w, _)| w)
    }

    fn latest() -> CurrentDriver {
        let latest = match read_latest_metadata().and_then(HashedHeapFile::from_metadata) {
            Ok(file) => Some(Rc::new(file)),
            Err(e) => {
                println!("preload: {}", e);
                None
            }
        };
        Rc::new(RefCell::new(latest))
    }
}
