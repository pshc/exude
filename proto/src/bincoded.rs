use std::io::{self, ErrorKind};
use std::marker::PhantomData;

use bincode;
use serde::{Deserialize, Serialize};

/// Holds the result of `bincode::serialize`.
#[derive(Clone)]
pub struct Bincoded<T> {
    vec: Vec<u8>,
    _phantom: PhantomData<T>,
}

pub static BINCODED_MAX: u64 = 0xffff;

fn to_io_err(err: bincode::Error) -> io::Error {
    match *err {
        bincode::ErrorKind::IoError(io) => io,
        e => io::Error::new(ErrorKind::Other, e)
    }
}

impl<T> Bincoded<T> {
    /// Returns the number of serialized bytes stored. Does not include length header.
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Not intended for general use; this is for low-level use.
    /// Precondition: `vec` must have been encoded with the same T.
    pub unsafe fn from_vec(vec: Vec<u8>) -> Self {
        Bincoded {vec: vec, _phantom: PhantomData}
    }
}

impl<T: Serialize> Bincoded<T> {
    /// Serializes `value`, storing the serialized bytes in `self`.
    pub fn new(value: &T) -> io::Result<Self> {
        let size_limit = bincode::Bounded(BINCODED_MAX);
        Ok(Bincoded {
            vec: bincode::serialize(value, size_limit).map_err(to_io_err)?,
            _phantom: PhantomData,
        })
    }
}

pub fn deserialize_exact<R, T>(slice: R) -> io::Result<T>
where
    R: AsRef<[u8]>,
    for<'de> T: Deserialize<'de>,
{
    let slice = slice.as_ref();
    let len = slice.len() as u64;
    let ref mut cursor = io::Cursor::new(slice);
    let result = bincode::deserialize_from(cursor, bincode::Infinite).map_err(to_io_err)?;

    // ensure the deserializer consumed every last byte
    if cursor.position() == len {
        Ok(result)
    } else {
        let msg = format!("extra bytes ({})", len - cursor.position());
        let io = io::Error::new(ErrorKind::InvalidData, msg);
        Err(io)
    }

}

impl<T> Bincoded<T> where for<'de> T: Deserialize<'de> {
    /// Deserialize the contained bytes.
    pub fn deserialize(&self) -> io::Result<T> {
        deserialize_exact(self)
    }
}

impl<T> AsRef<[u8]> for Bincoded<T> {
    fn as_ref(&self) -> &[u8] {
        self.vec.as_ref()
    }
}

impl<T> Into<Vec<u8>> for Bincoded<T> {
    fn into(self) -> Vec<u8> {
        self.vec
    }
}

#[test]
fn roundtrip() {
    let orig = (42, format!("hello"));
    let coded = Bincoded::new(&orig).unwrap().deserialize().unwrap();
    assert_eq!(orig, coded);
}

#[test]
fn too_short() {
    let short: Bincoded<u32> = Bincoded {vec: vec![1, 2, 3], _phantom: PhantomData};
    let err = short.deserialize().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn too_long() {
    use std::iter;
    use std::error::Error;
    let bytes: Vec<u8> = iter::repeat(0).take(17).collect();
    let long: Bincoded<(u64, u64)> = Bincoded {vec: bytes, _phantom: PhantomData};
    let err = long.deserialize().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(err.description(), "extra bytes (1)");
}
