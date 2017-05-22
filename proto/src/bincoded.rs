use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::marker::PhantomData;
use std::path::Path;

use bincode;
use serde::{Deserialize, Serialize};

pub use bincode::{Error, ErrorKind, Result};

/// Holds the result of `bincode::serialize`.
#[derive(Clone)]
pub struct Bincoded<T> {
    vec: Vec<u8>,
    _phantom: PhantomData<T>,
}

pub static BINCODED_MAX: u64 = 0xffff;

impl<T> Bincoded<T> {
    /// Returns the number of serialized bytes stored. Does not include length header.
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Not intended for general use; this is for low-level use.
    /// Precondition: `vec` must have been encoded with the same T.
    pub unsafe fn from_vec(vec: Vec<u8>) -> Self {
        assert!(!vec.is_empty());
        Bincoded { vec, _phantom: PhantomData }
    }
}

impl<T: Serialize> Bincoded<T> {
    /// Serializes `value`, storing the serialized bytes in `self`.
    pub fn new(value: &T) -> Result<Self> {
        let size_limit = bincode::Bounded(BINCODED_MAX);
        let vec = bincode::serialize(value, size_limit)?;
        debug_assert!(
            !vec.is_empty(),
            "zero-length {}",
            unsafe { ::std::intrinsics::type_name::<T>() },
        );
        assert!(!vec.is_empty(), "zero-length serialization");
        Ok(Bincoded { vec, _phantom: PhantomData })
    }

    /// Does not write a header or anything fancy.
    pub fn write_to_path(&self, path: &Path) -> io::Result<()> {
        let mut f = File::create(path)?;
        f.write_all(&self.vec)?;
        f.sync_all()
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
        let mut vec = Vec::new();
        File::open(path)?.read_to_end(&mut vec)?;
        if vec.is_empty() {
            debug_assert!(
                false,
                "zero-length {} {}",
                ::std::intrinsics::type_name::<T>(),
                path.display(),
            );
            Err(io::Error::new(io::ErrorKind::InvalidData, "zero-length deserialization"))
        } else {
            Ok(Bincoded { vec: vec, _phantom: PhantomData })
        }
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

#[cfg(test)]
mod test {
    extern crate tempfile;

    use std::io;
    use std::iter;
    use std::marker::PhantomData;

    use bincode::ErrorKind;
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
        let short: Bincoded<u32> = Bincoded { vec: vec![1, 2, 3], _phantom: PhantomData };
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
        let bytes: Vec<u8> = iter::repeat(0).take(17).collect();
        let long: Bincoded<(u64, u64)> = Bincoded { vec: bytes, _phantom: PhantomData };
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
