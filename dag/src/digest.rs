use std::fmt::{self, Debug, Display};
use std::io;
use std::io::prelude::*;
use std::str::{self, FromStr};

use digest_crate::{Input, VariableOutput};
use sha3::Shake128;

pub static HEX_CHARS: &[u8] = b"0123456789abcdef";
pub const LEN: usize = 32;

/// Stores a 256-bit hash digest.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Digest(pub [u8; LEN]);

impl Digest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Shake128::default();
        hasher.process(bytes);
        let mut result = [0u8; LEN];
        hasher.variable_result(&mut result).expect("hashing");
        Digest(result)
    }

    pub fn from_read<R: Read>(mut reader: R) -> io::Result<(Digest, usize)> {
        let mut hasher = Shake128::default();
        let mut buf = [0; 4096];
        let mut len = 0;

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    len += n;
                    hasher.process(&buf[..n]);
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => (),
                Err(e) => return Err(e),
            }
        }

        let mut result = [0u8; LEN];
        hasher.variable_result(&mut result).expect("hashing");
        Ok((Digest(result), len))
    }

    /// Always returns valid ASCII.
    pub fn hex_bytes(&self) -> [u8; LEN * 2] {
        let mut ascii = [b'x'; LEN * 2];
        for (i, octet) in self.0.iter().enumerate() {
            ascii[i * 2] = HEX_CHARS[(octet >> 4) as usize];
            ascii[i * 2 + 1] = HEX_CHARS[(octet & 0x0f) as usize];
        }
        ascii
    }

    pub fn short_hex(&self) -> String {
        let ascii = self.hex_bytes();
        let hex = unsafe { str::from_utf8_unchecked(&ascii[..12]) };
        hex.to_owned()
    }

    pub fn zero() -> Self {
        Digest([0; LEN])
    }

    #[cfg(test)]
    pub fn sample() -> Self {
        let mut bytes = [0x33; LEN];
        bytes[1] = 0x55;
        bytes[12] = 0x23;
        bytes[LEN - 2] = 0xf0;
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
        let ascii = self.hex_bytes();
        let hex = unsafe { str::from_utf8_unchecked(&ascii) };
        f.write_str(&hex)
    }
}

impl FromStr for Digest {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        if s.len() != LEN * 2 {
            return Err(())
        }
        let mut bytes = [0u8; LEN];
        let mut ascii = s.bytes();
        for byte in bytes.iter_mut() {
            let hi = ascii.next().unwrap();
            let lo = ascii.next().unwrap();
            if hi > b'f' || lo > b'f' {
                return Err(())
            }
            let pair = [hi, lo];
            let utf8 = unsafe { str::from_utf8_unchecked(&pair) };
            *byte = u8::from_str_radix(utf8, 16).map_err(|_| ())?
        }
        Ok(Digest(bytes))
    }
}

#[test]
fn hex() {
    let digest = Digest([0xff; LEN]);
    let hex = format!("{}", digest);
    assert_eq!(hex.len(), LEN * 2);
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
fn parse() {
    assert_eq!(
        "0000000000000000000000000000000000000000000000000000000000000000".parse::<Digest>(),
        Ok(Digest::zero())
    );

    let a: Digest = "0123456789abcdef02468ace13579bdf000102030405060708090a0b0c0d0e0f"
        .parse().unwrap();
    let b = Digest([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0x02, 0x46, 0x8a, 0xce, 0x13, 0x57, 0x9b, 0xdf,
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    ]);
    assert_eq!(a, b);

    assert_eq!("0000".parse::<Digest>(), Err(()));
    assert_eq!(
        "000000000000000000000000000000000000000000000000000000000000000x".parse::<Digest>(),
        Err(())
    );
}

#[test]
fn roundtrip() {
    // this is an inadvertent integration test!
    // should use an independent serializer
    use bincoded::Bincoded;

    let orig = Digest::sample();
    let there_and_back_again = Bincoded::new(&orig).unwrap().deserialize().unwrap();
    assert_eq!(orig, there_and_back_again);
}

#[test]
fn bincoded_repr() {
    use bincoded::Bincoded;

    let orig = Digest::sample();
    let coded = Bincoded::new(&orig).expect("bincode digest");
    assert_eq!(coded.as_ref().len(), LEN);
    assert_eq!(&orig.0[..], coded.as_ref());
}
