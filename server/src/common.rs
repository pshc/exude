//! Shared messaging code between client and server.

use futures::future::{self, Future};
use tokio_io::{self, AsyncRead, AsyncWrite};

use errors::*;
use proto::Bincoded;
use proto::serde::{Deserialize, Serialize};


/// No `Send` needed.
pub type OurFuture<T> = Box<Future<Item = T, Error = Error>>;

/// Reads a 16-bit length header and then bytes asynchronously.
pub fn read_with_length<R: AsyncRead + 'static>(reader: R) -> OurFuture<(R, Vec<u8>)> {
    let buf = [0u8, 0];
    box tokio_io::io::read_exact(reader, buf)
            .and_then(
        |(reader, len_buf)| {
            let len = ((len_buf[0] as usize) << 8) | len_buf[1] as usize;
            let mut buf = Vec::with_capacity(len);
            unsafe {
                buf.set_len(len);
            }
            tokio_io::io::read_exact(reader, buf)
        },
    )
            .then(|res| res.chain_err(|| "couldn't read length-delimited packet"),)
}

/// Reads a 16-bit length header and then buffers and deserializes bytes.
pub fn read_bincoded<R, T>(reader: R) -> OurFuture<(R, T)>
where
    R: AsyncRead + 'static,
    for<'de> T: Deserialize<'de> + 'static,
{
    box read_with_length(reader).and_then(
        |(reader, vec)| {
            let bincoded = unsafe { Bincoded::<T>::from_vec(vec) };
            bincoded
                .deserialize()
                .map(|val| (reader, val))
                .map_err(Into::into)
        },
    )
}

/// Write a 16-bit length header, and then the bytes asynchronously.
///
/// The future returns `(write_half, vec)`.
pub fn write_with_length<W, V>(writer: W, vec: V) -> OurFuture<(W, V)>
where
    W: AsyncWrite + 'static,
    V: AsRef<[u8]> + 'static,
{
    let len = vec.as_ref().len();
    if len > 0xffff {
        return box future::err(format!("written message too long: {}", len).into());
    }
    let len_buf = [(len >> 8) as u8, len as u8];
    box tokio_io::io::write_all(writer, len_buf)
            .and_then(move |(writer, _)| tokio_io::io::write_all(writer, vec))
            .then(|res| res.chain_err(|| "couldn't write length-delimited packet"),)
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
