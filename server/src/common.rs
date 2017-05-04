//! Shared messaging code between client and server.

use std::io::{self, ErrorKind};

use futures::future::{self, Future};
use tokio_io::{self, AsyncRead, AsyncWrite};

use proto::Bincoded;
use proto::serde::{Deserialize, Serialize};


/// No `Send` needed.
pub type IoFuture<T> = Box<Future<Item=T, Error=io::Error>>;

/// Reads a 16-bit length header and then bytes asynchronously.
pub fn read_with_length<R: AsyncRead + 'static>(reader: R) -> IoFuture<(R, Vec<u8>)> {
    let buf = [0u8, 0];
    box tokio_io::io::read_exact(reader, buf).and_then(|(reader, len_buf)| {
        let len = ((len_buf[0] as usize) << 8) | len_buf[1] as usize;
        let mut buf = Vec::with_capacity(len);
        unsafe {
            buf.set_len(len);
        }
        tokio_io::io::read_exact(reader, buf)
    })
}

/// Reads a 16-bit length header and then buffers and deserializes bytes.
pub fn read_bincoded<R, T>(reader: R) -> IoFuture<(R, T)>
where
    R: AsyncRead + 'static,
    for<'de> T: Deserialize<'de> + 'static,
{
    box read_with_length(reader).and_then(|(reader, vec)| {
        let bincoded = unsafe { Bincoded::<T>::from_vec(vec) };
        match bincoded.deserialize() {
            Ok(val) => Ok((reader, val)),
            Err(e) => Err(io::Error::new(ErrorKind::InvalidData, e))
        }
    })
}

/// Write a 16-bit length header, and then the bytes asynchronously.
///
/// The future returns `(write_half, vec)`.
pub fn write_with_length<W, V>(writer: W, vec: V) -> IoFuture<(W, V)>
where
    W: AsyncWrite + 'static,
    V: AsRef<[u8]> + 'static,
{
    let len = vec.as_ref().len();
    box future::lazy(move || {
        if len <= 0xffff {
            Ok([(len >> 8) as u8, len as u8])
        } else {
            Err(io::Error::new(ErrorKind::InvalidInput, format!("msg too long: {}", len)))
        }
    })
    .and_then(move |len_buf| tokio_io::io::write_all(writer, len_buf))
    .and_then(move |(writer, _)| tokio_io::io::write_all(writer, vec))
}

/// Write a 16-bit length header, and then the serialized bytes.
pub fn write_bincoded<W, T>(writer: W, value: &T) -> IoFuture<(W, Bincoded<T>)>
where
    W: AsyncWrite + 'static,
    T: Serialize + 'static,
{
    match Bincoded::new(value) {
        Ok(bincoded) => write_with_length(writer, bincoded),
        Err(e) => box future::err(io::Error::new(ErrorKind::InvalidInput, e))
    }
}
