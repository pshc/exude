use std::sync::mpsc;

use common::{self, OurFuture};
use errors::*;
use futures::future::Future;
use futures::stream::{self, Stream};
use futures::sync::mpsc::UnboundedReceiver;
use tokio_io::{AsyncRead, AsyncWrite};

use proto::Bytes;

pub struct Comms {
    pub tx: mpsc::Sender<Bytes>,
    pub rx: UnboundedReceiver<Bytes>,
}

impl Comms {
    pub fn handle<R, W>(self, r: R, w: W) -> OurFuture<()>
    where
        R: AsyncRead + 'static,
        W: AsyncWrite + 'static,
    {
        let Comms { tx, rx } = self;

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
