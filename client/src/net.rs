use std::sync::mpsc;

use common::{self, OurFuture};
use errors::*;
use futures::{self, Future, Stream};
use tokio_io::{AsyncRead, AsyncWrite};

pub struct Comms {
    pub tx: mpsc::Sender<Box<[u8]>>,
    pub rx: futures::sync::mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl Comms {
    pub fn handle<R, W>(self, r: R, w: W) -> OurFuture<(R, W)>
    where
        R: AsyncRead + 'static,
        W: AsyncWrite + 'static,
    {
        let Comms { tx, rx } = self;

        // todo: read more than one message
        let read = common::read_with_length(r).and_then(
            move |(r, vec)| {
                tx.send(vec.into_boxed_slice())
                    .chain_err(|| ErrorKind::BrokenComms)
                    .map(|_| r)
            }
        );

        let write = rx
        .map_err(|()| ErrorKind::BrokenComms.into())
        .fold(w, |w, msg| {
            common::write_with_length(w, msg).map(|(w, _)| w)
        });

        box read.join(write)
    }
}
