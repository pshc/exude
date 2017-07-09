use std::fs::{self, File};
use std::io;
use std::io::prelude::*;
use std::marker::PhantomData;
use std::path::Path;

use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

pub use bincode::{self, Error, ErrorKind, Result};

/// Holds the result of `bincode::serialize`.
#[derive(Clone)]
pub struct Bincoded<T> {
    bytes: Bytes,
    _phantom: PhantomData<T>,
}

pub static BINCODED_MAX: u64 = 0xffff;

impl<T> Bincoded<T> {
    /// Returns the number of serialized bytes stored. Does not include length header.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Not intended for general use; this is for low-level use.
    /// Precondition: `vec` must have been encoded with the same T.
    pub unsafe fn from_bytes(bytes: Bytes) -> Self {
        assert!(!bytes.is_empty());
        Bincoded { bytes, _phantom: PhantomData }
    }
}

impl<T: Serialize> Bincoded<T> {
    /// Serializes `value`, storing the serialized bytes in `self`.
    pub fn new(value: &T) -> Result<Self> {
        // first do a pass to determine the message length
        // this may be slower than simply writing to a growing vec...
        // waiting on https://github.com/carllerche/bytes/issues/77
        let len = serialized_size(value)?;
        // now actually serialize the bytes
        let mut bytes = BytesMut::with_capacity(len).writer();
        bincode::serialize_into(&mut bytes, value, bincode::Infinite)?;
        let bytes = bytes.into_inner().freeze();
        Ok(Bincoded { bytes, _phantom: PhantomData })
    }

    /// Does not write a header or anything fancy.
    pub fn write_to_path(&self, path: &Path) -> io::Result<()> {
        let mut f = File::create(path)?;
        f.write_all(&self.bytes)?;
        f.sync_all()
    }
}

pub fn serialized_size<T: Serialize>(value: &T) -> Result<usize> {
    match bincode::serialized_size_bounded(value, BINCODED_MAX) {
        Some(0) => {
            debug_assert!(
                false,
                "zero-length {}",
                unsafe { ::std::intrinsics::type_name::<T>() },
            );
            panic!("zero-length serialization");
        }
        Some(len) => Ok(len as usize), // unchecked cast
        None => {
            debug_assert!(
                false,
                "{} is too long",
                unsafe { ::std::intrinsics::type_name::<T>() },
            );
            Err(Box::new(bincode::ErrorKind::SizeLimit))
        }
    }
}

pub fn deserialize_exact<R, T>(slice: R) -> Result<T>
where
    R: AsRef<[u8]>,
    for<'de> T: Deserialize<'de>,
{
    let slice = slice.as_ref();
    let len = slice.len() as u64;
    let ref mut cursor = io::Cursor::new(slice);
    let result = bincode::deserialize_from(cursor, bincode::Infinite)?;

    // ensure the deserializer consumed every last byte
    if cursor.position() == len {
        Ok(result)
    } else {
        let msg = format!("extra bytes ({})", len - cursor.position());
        let io = io::Error::new(io::ErrorKind::InvalidData, msg);
        Err(Box::new(ErrorKind::IoError(io)))
    }
}

impl<T> Bincoded<T>
where
    for<'de> T: Deserialize<'de>,
{
    /// Deserialize the contained bytes.
    pub fn deserialize(&self) -> Result<T> {
        deserialize_exact(self)
    }

    /// Deserialize from the given file.
    /// Precondition: file at `path` must have been encoded with the same T.
    pub unsafe fn from_path(path: &Path) -> io::Result<Self> {
        let len = fs::metadata(path)?.len() as usize; // unchecked cast
        if len == 0 {
            debug_assert!(
                false,
                "zero-length {} {}",
                ::std::intrinsics::type_name::<T>(),
                path.display(),
            );
            return Err(io::Error::new(io::ErrorKind::InvalidData, "zero-length deserialization"));
        }
        let mut bytes = BytesMut::with_capacity(len);
        bytes.set_len(len); // unsafe
        let mut f = File::open(path)?;
        f.read_exact(&mut bytes)?;
        // ensure we hit eof
        loop {
            match f.read(&mut [0]) {
                Ok(0) => break,
                Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "not eof")),
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(Bincoded { bytes: bytes.freeze(), _phantom: PhantomData })
    }
}

impl<T> AsRef<[u8]> for Bincoded<T> {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

impl<T> Into<Bytes> for Bincoded<T> {
    fn into(self) -> Bytes {
        self.bytes
    }
}

#[cfg(test)]
mod test {
    extern crate tempfile;

    use std::io;
    use std::marker::PhantomData;

    use bincode::ErrorKind;
    use bytes::Bytes;
    use self::tempfile::NamedTempFile;

    use super::Bincoded;

    #[test]
    fn roundtrip() {
        let orig = (42, format!("hello"));
        let coded = Bincoded::new(&orig).unwrap().deserialize().unwrap();
        assert_eq!(orig, coded);
    }

    #[test]
    fn too_short() {
        let bytes = Bytes::from(&[1u8, 2, 3][..]);
        let short: Bincoded<u32> = Bincoded { bytes, _phantom: PhantomData };
        match *short.deserialize().unwrap_err() {
            ErrorKind::IoError(io) => {
                assert_eq!(io.kind(), io::ErrorKind::UnexpectedEof);
            }
            e => panic!(e),
        }
    }

    #[test]
    fn too_long() {
        use std::error::Error;
        let bytes = Bytes::from(&[0u8; 17][..]);
        let long: Bincoded<(u64, u64)> = Bincoded { bytes, _phantom: PhantomData };
        match *long.deserialize().unwrap_err() {
            ErrorKind::IoError(io) => {
                assert_eq!(io.kind(), io::ErrorKind::InvalidData);
                assert_eq!(io.description(), "extra bytes (1)");
            }
            e => panic!(e),
        }
    }

    #[test]
    #[should_panic]
    fn zero() {
        Bincoded::new(&()).unwrap();
    }

    #[test]
    #[should_panic]
    fn zero_file() {
        let tmp = NamedTempFile::new().unwrap(); // do not use NamedTempFile in real code!
        unsafe { Bincoded::<()>::from_path(tmp.path()).unwrap() };
    }

    #[test]
    fn file() {
        let orig = (format!("route"), 66);
        let tmp = NamedTempFile::new().unwrap(); // do not use NamedTempFile in real code!
        let coded = Bincoded::new(&orig).unwrap();
        coded.write_to_path(tmp.path()).unwrap();
        let read = unsafe { Bincoded::from_path(tmp.path()).unwrap() };
        let decoded = read.deserialize().unwrap();
        assert_eq!(orig, decoded);
    }
}
