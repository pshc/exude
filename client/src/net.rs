use std::io::{self, ErrorKind};
use std::sync::mpsc;

use common::{self, IoFuture};
use futures::{self, Future, Stream};
use tokio_io::{AsyncRead, AsyncWrite};

pub struct Comms {
    pub tx: mpsc::Sender<Box<[u8]>>,
    pub rx: futures::sync::mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl Comms {
    pub fn handle<R, W>(self, r: R, w: W) -> IoFuture<(R, W)>
    where
        R: AsyncRead + 'static,
        W: AsyncWrite + 'static,
    {
        let Comms { tx, rx } = self;

        // todo: read more than one message
        let read = common::read_with_length(r).and_then(
            move |(r, vec)| {
                tx.send(vec.into_boxed_slice())
                    .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "core: done reading"),)
                    .map(|_| r)
            },
        );

        let write = rx
        .map_err(|()| io::Error::new(ErrorKind::BrokenPipe, "core: done writing"))
        .fold(w, |w, msg| {
            common::write_with_length(w, msg).map(|(w, _)| w)
        });

        box read.join(write)
    }
}
