//! Shared messaging code between client and server.

use futures::future::{self, Future};
use tokio_io::{self, AsyncRead, AsyncWrite};

use errors::*;
use proto::{Bincoded, BytesMut};
use proto::serde::{Deserialize, Serialize};


/// hopefully replace with `?` later
#[macro_export]
macro_rules! try_box {
    ($expr:expr) => (match $expr {
        Ok(val) => val,
        Err(err) => {
            let err = Err(From::from(err));
            let future = ::futures::future::IntoFuture::into_future(err);
            return Box::new(future);
        }
    })
}

/// No `Send` needed.
pub type OurFuture<T> = Box<Future<Item = T, Error = Error>>;

/// Reads a 16-bit length header and then bytes asynchronously.
pub fn read_with_length<R: AsyncRead + 'static>(reader: R) -> OurFuture<(R, BytesMut)> {
    let buf = [0u8, 0];
    box tokio_io::io::read_exact(reader, buf)
            .and_then(
        |(reader, len_buf)| {
            let len = ((len_buf[0] as usize) << 8) | len_buf[1] as usize;
            let mut bytes = BytesMut::with_capacity(len);
            unsafe {
                bytes.set_len(len);
            }
            tokio_io::io::read_exact(reader, BytesMutAsMut(bytes))
        }
    )
            .then(|res| {
                res
                    .map(|(r, bmam)| (r, bmam.0))
                    .chain_err(|| "couldn't read length-delimited packet")
            })
}

// BytesMut does not impl AsMut<[u8]>; workaround
struct BytesMutAsMut(BytesMut);

impl AsMut<[u8]> for BytesMutAsMut {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

/// Reads a 16-bit length header and then buffers and deserializes bytes.
pub fn read_bincoded<R, T>(reader: R) -> OurFuture<(R, T)>
where
    R: AsyncRead + 'static,
    for<'de> T: Deserialize<'de> + 'static,
{
    box read_with_length(reader).and_then(
        |(reader, bytes)| {
            let bincoded = unsafe { Bincoded::<T>::from_bytes(bytes.freeze()) };
            bincoded
                .deserialize()
                .map(|val| (reader, val))
                .map_err(Into::into)
        }
    )
}

/// Write a 16-bit length header, and then `buf` asynchronously.
///
/// The future returns `(write_half, buf)`.
pub fn write_with_length<W, B>(writer: W, buf: B) -> OurFuture<(W, B)>
where
    W: AsyncWrite + 'static,
    B: AsRef<[u8]> + 'static,
{
    let len = buf.as_ref().len();
    if len > 0xffff {
        return box future::err(format!("written message too long: {}", len).into());
    }
    let len_buf = [(len >> 8) as u8, len as u8];
    box tokio_io::io::write_all(writer, len_buf)
            .and_then(move |(writer, _)| tokio_io::io::write_all(writer, buf))
            .then(|res| res.chain_err(|| "couldn't write length-delimited packet"))
}

/// Write a 16-bit length header, and then the serialized bytes.
pub fn write_bincoded<W, T>(writer: W, value: &T) -> OurFuture<(W, Bincoded<T>)>
where
    W: AsyncWrite + 'static,
    T: Serialize + 'static,
{
    match Bincoded::new(value) {
        Ok(bincoded) => write_with_length(writer, bincoded),
        Err(e) => box future::err(e.into()),
    }
}
