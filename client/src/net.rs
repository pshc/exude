use std::time::Duration;
use std::path::PathBuf;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex, mpsc};

use common::{self, OurFuture};
use errors::*;
use futures::future::{self, Future, Loop};
use futures::stream::{self, Stream};
use futures::sync::mpsc::UnboundedReceiver;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;
use tokio_timer;

use proto::Bytes;

pub fn thread<H>(server_addr: SocketAddr, handshake: H)
where
    H: FnMut(TcpStream) -> OurFuture<(TcpStream, ClientSide)>,
{
    let mut core = Core::new().expect("net: core");
    let handle = core.handle();

    // since we're only using this for reconnects (so far...)
    // allocate only a modest amount
    let timer = tokio_timer::wheel()
        .tick_duration(Duration::from_millis(500))
        .num_slots(8) // max timeout only four seconds!
        .initial_capacity(8)
        .channel_capacity(8)
        .thread_name("net timer")
        .build();
    let reconnect_delay = Duration::from_secs(2);

    // is this mutex really necessary?
    // i think a trait with a handshake method wouldn't need one...
    let handshake = Arc::new(Mutex::new(handshake));

    let client = future::loop_fn(0, move |attempt| {
        let handshake = handshake.clone();
        let timer = timer.clone();

        println!("net: connecting...");
        TcpStream::connect(&server_addr, &handle)
        .then(|res| res.chain_err(|| format!("couldn't connect to server")))
        .and_then(
            move |sock| {
                let mut handshake = handshake.lock().expect("lock handshake");
                handshake.deref_mut()(sock)
                    .and_then(|(sock, handler)| handler.handle(sock))
                    .map(Loop::Break)
            }
        )
        .or_else(move |e| -> Box<Future<Item = _, Error = ()>> {
            display_net_thread_error(e).expect("net: stderr?");
            if attempt < 3 {
                box timer.sleep(reconnect_delay)
                    .or_else(|e| { println!("timer error: {:?}", e); Ok(()) })
                    .map(move |()| Loop::Continue(attempt + 1))
            } else {
                box future::err(())
            }
        })
    });

    match core.run(client) {
        Ok(()) => println!("net: donezo"),
        Err(()) => println!("net: too many failures"),
    }
}

pub type DriverUpdate<D> = (PathBuf, Box<D>);

pub struct ClientSide {
    pub tx: mpsc::Sender<Bytes>,
    pub rx: UnboundedReceiver<Bytes>,
}

impl ClientSide {
    pub fn handle(self, sock: TcpStream) -> OurFuture<()> {
        let (r, w) = sock.split();
        let ClientSide { tx, rx } = self;

        fn swap<A, B>((a, b): (A, B)) -> (B, A) {
            (b, a)
        }

        let read = stream::unfold(r, |r| Some(common::read_with_length(r).map(swap)))
            .for_each(
                move |bytes| {
                    tx.send(bytes.freeze())
                        .chain_err(|| ErrorKind::BrokenComms)
                }
            );

        let write = rx
        .map_err(|()| ErrorKind::BrokenComms.into())
        .fold(w, |w, msg| {
            common::write_with_length(w, msg).map(|(w, _)| w)
        });

        box read.join(write).map(|((), _w)| ())
    }
}
