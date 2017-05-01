use std::fmt::{self, Debug, Display};
use std::str;

pub const LEN: usize = 32;

/// Stores a 256-bit hash digest.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Digest(pub [u8; LEN]);

impl Digest {
    /// Always returns valid ASCII.
    pub fn hex_bytes(&self) -> [u8; LEN*2] {
        let mut ascii = [b'x'; LEN*2];
        static HEX: &[u8] = b"0123456789abcdef";
        for (i, octet) in self.0.iter().enumerate() {
            ascii[i*2] = HEX[(octet >> 4) as usize];
            ascii[i*2+1] = HEX[(octet & 0x0f) as usize];
        }
        ascii
    }

    #[allow(dead_code)]
    pub fn short_hex(&self) -> String {
        let ascii = self.hex_bytes();
        let hex = unsafe { str::from_utf8_unchecked(&ascii[..12]) };
        hex.to_owned()
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Digest([0; LEN])
    }

    #[cfg(test)]
    pub fn sample() -> Self {
        let mut bytes = [0x33; LEN];
        bytes[1] = 0x55;
        bytes[12] = 0x23;
        bytes[LEN-2] = 0xf0;
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

#[test]
fn hex() {
    let digest = Digest([0xff; LEN]);
    let hex = format!("{}", digest);
    assert_eq!(hex.len(), LEN*2);
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
    use super::Bincoded;

    let orig = Digest::sample();
    let there_and_back_again = Bincoded::new(&orig).unwrap().deserialize().unwrap();
    assert_eq!(orig, there_and_back_again);
}

#[test]
fn bincoded_repr() {
    use super::Bincoded;

    let orig = Digest::sample();
    let coded = Bincoded::new(&orig).unwrap();
    assert_eq!(coded.as_ref().len(), LEN);
    assert_eq!(&orig.0[..], coded.as_ref());
}

