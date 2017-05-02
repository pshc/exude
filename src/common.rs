//! Shared code between client and server.

#![allow(dead_code)]

use std::io::{self, ErrorKind};

use futures::future::{self, Future};
use serde::{Deserialize, Serialize};
use tokio_io::{self, AsyncRead, AsyncWrite};

use proto::{Bincoded, Digest, DriverInfo};

#[derive(Debug, Deserialize, Serialize)]
pub enum Welcome {
    Current,
    InlineDriver(DriverInfo),
    DownloadDriver(String, DriverInfo),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Hello(pub Option<Digest>);

/// Reads a 16-bit length header and then bytes asynchronously.
pub fn read_with_length<R: AsyncRead>(reader: R)
-> impl Future<Item=(R, Vec<u8>), Error=io::Error>
{
    let buf = [0u8, 0];
    tokio_io::io::read_exact(reader, buf).and_then(|(reader, len_buf)| {
        let len = ((len_buf[0] as usize) << 8) | len_buf[1] as usize;
        let mut buf = Vec::with_capacity(len);
        unsafe {
            buf.set_len(len);
        }
        tokio_io::io::read_exact(reader, buf)
    })
}

/// Reads a 16-bit length header and then buffers and deserializes bytes.
pub fn read_bincoded<R: AsyncRead, T: Deserialize>(reader: R)
-> impl Future<Item=(R, T), Error=io::Error>
{
    read_with_length(reader).and_then(|(reader, vec)| {
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
pub fn write_with_length<W: AsyncWrite, V: AsRef<[u8]>>(writer: W, vec: V)
-> impl Future<Item=(W, V), Error=io::Error>
{
    let len = vec.as_ref().len();
    future::lazy(move || {
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
pub fn write_bincoded<W: AsyncWrite + 'static, T: Serialize + 'static>(writer: W, value: &T)
-> Box<Future<Item=(W, Bincoded<T>), Error=io::Error>>
{
    match Bincoded::new(value) {
        Ok(bincoded) => box write_with_length(writer, bincoded),
        Err(e) => box future::err(io::Error::new(ErrorKind::InvalidInput, e))
    }
}
