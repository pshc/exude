//! Shared code between client and server.

#![allow(dead_code)]

use std::io::{self, ErrorKind};

use futures::future::Future;
use serde::{Deserialize, Serialize};
use tokio_io::{self, AsyncRead, AsyncWrite};

use env::bincoded::Bincoded;

pub use self::digest::Digest;

#[derive(Debug, Deserialize, Serialize)]
pub enum Welcome {
    Current,
    InlineDriver(u32, Digest),
    DownloadDriver(String, Digest),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Hello(pub Option<Digest>);

#[derive(Debug, Deserialize, Serialize)]
pub enum UpRequest {
    Ping(u32),
    Bye,
}

mod digest {
    use std::fmt::{self, Debug, Display, Write};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde::de;

    /// Stores a 512-bit hash digest.
    pub struct Digest(pub [u8; 64]);

    struct DigestVisitor;
    impl de::Visitor for DigestVisitor {
        type Value = Digest;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "a SHAKE256 digest")
        }

        fn visit_seq<V: de::SeqVisitor>(self, mut visitor: V) -> Result<Self::Value, V::Error> {
            let mut bytes = [0u8; 64];
            for i in 0..64 {
                if let Some(byte) = visitor.visit()? {
                    bytes[i] = byte
                } else {
                    use serde::de::Error;
                    return Err(V::Error::invalid_length(i, &self))
                }
            }
            Ok(Digest(bytes))
        }
    }

    impl Deserialize for Digest {
        fn deserialize<D: Deserializer>(d: D) -> Result<Self, D::Error> {
            d.deserialize_seq_fixed_size(64, DigestVisitor)
        }
    }

    impl Serialize for Digest {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            debug_assert_eq!(self.0.len(), 64);
            let mut seq = s.serialize_seq_fixed_size(64)?;
            for byte in self.0.iter() {
                seq.serialize_element(byte)?;
            }
            seq.end()
        }
    }

    impl Clone for Digest {
        fn clone(&self) -> Self {
            let mut bytes = [0u8; 64];
            bytes.copy_from_slice(&self.0[..]);
            Digest(bytes)
        }
    }

    impl PartialEq for Digest {
        fn eq(&self, other: &Digest) -> bool {
            return self.0[..] == other.0[..]
        }
    }
    impl Eq for Digest {}

    impl Digest {
        pub fn short_hex(&self) -> String {
            let mut hex = String::with_capacity(12);
            for octet in self.0.iter().take(6) {
                write!(hex, "{:02x}", octet).unwrap();
            }
            hex
        }

        #[cfg(test)]
        pub fn zero() -> Self {
            Digest([0; 64])
        }

        #[cfg(test)]
        pub fn sample() -> Self {
            let mut bytes = [0x33; 64];
            bytes[1] = 0x55;
            bytes[12] = 0x23;
            bytes[50] = 0xf0;
            Digest(bytes)
        }
    }

    impl Debug for Digest {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "Digest({})", self)
        }
    }

    impl Display for Digest {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            for octet in self.0.iter() {
                write!(f, "{:02x}", octet)?;
            }
            Ok(())
        }
    }

    #[test]
    fn hex() {
        let digest = Digest([0xff; 64]);
        let hex = format!("{}", digest);
        assert_eq!(hex.len(), 128);
        for b in hex.bytes() {
            assert_eq!(b, b'f');
        }
        assert_eq!(digest.short_hex(), "ffffffffffff");
    }

    #[test]
    fn eq() {
        let x = Digest::sample();
        let z = Digest::zero();
        assert_eq!(x, x);
        assert_eq!(x, x.clone());
        assert_eq!(z, z);
        assert!(x != z && z != x);
    }

    #[test]
    fn roundtrip() {
        // this is an inadvertent integration test!
        // should use an independent serializer
        use ::env::bincoded::Bincoded;

        let orig = Digest::sample();
        let there_and_back_again = Bincoded::new(&orig).unwrap().deserialize().unwrap();
        assert_eq!(orig, there_and_back_again);
    }

    #[test]
    fn bincoded_repr() {
        use ::env::bincoded::Bincoded;

        let orig = Digest::sample();
        let coded = Bincoded::new(&orig).unwrap();
        assert_eq!(coded.as_ref().len(), 64);
        assert_eq!(&orig.0[..], coded.as_ref());
    }
}

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
pub fn write_with_length<W: AsyncWrite, V: Into<Vec<u8>>>(writer: W, vec: V)
-> impl Future<Item=(W, Vec<u8>), Error=io::Error>
{
    let vec = vec.into();
    let len = vec.len();
    assert!(len <= 0xffff); // xxx should return a descriptive error or something?
    let len_buf = [(len >> 8) as u8, len as u8];
    tokio_io::io::write_all(writer, len_buf)
        .and_then(|(writer, _)| tokio_io::io::write_all(writer, vec))
}

/// Write a 16-bit length header, and then the serialized bytes.
pub fn write_bincoded<W: AsyncWrite + 'static, T: Serialize + 'static>(writer: W, value: &T)
-> Box<Future<Item=(W, Bincoded<T>), Error=io::Error>>
{
    match Bincoded::new(value) {
        Ok(bincoded) => {
            // it's awkward that we immediately unwrap and rewrap the Bincoded vec...
            box write_with_length(writer, bincoded)
                .map(|(w, vec)| (w, unsafe { Bincoded::from_vec(vec) }))
        }
        Err(e) => box ::futures::future::err(io::Error::new(ErrorKind::InvalidInput, e))
    }

}
