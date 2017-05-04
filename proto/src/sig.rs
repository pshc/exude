use std::fmt::{self, Debug, Display};
use std::str;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de;

use super::HEX_CHARS;

pub const LEN: usize = 64;

/// Stores a 512-bit sodiumoxide signature.
pub struct Signature(pub [u8; LEN]);

struct Visitor;
impl<'de> de::Visitor<'de> for Visitor {
    type Value = Signature;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a sodium signature")
    }

    fn visit_seq<V: de::SeqAccess<'de>>(self, mut visitor: V) -> Result<Self::Value, V::Error> {
        let mut bytes = [0u8; LEN];
        for i in 0..LEN {
            if let Some(byte) = visitor.next_element()? {
                bytes[i] = byte
            } else {
                use serde::de::Error;
                return Err(V::Error::invalid_length(i, &self))
            }
        }
        Ok(Signature(bytes))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_tuple(LEN, Visitor)
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        debug_assert_eq!(self.0.len(), LEN);
        let mut seq = s.serialize_tuple(LEN)?;
        for byte in self.0.iter() {
            seq.serialize_element(byte)?;
        }
        seq.end()
    }
}

impl Clone for Signature {
    fn clone(&self) -> Self {
        let mut bytes = [0u8; LEN];
        bytes.copy_from_slice(&self.0[..]);
        Signature(bytes)
    }
}

impl PartialEq for Signature {
    fn eq(&self, other: &Signature) -> bool {
        return self.0[..] == other.0[..]
    }
}
impl Eq for Signature {}

impl Signature {
    /// Always returns valid ASCII.
    pub fn hex_bytes(&self) -> [u8; LEN*2] {
        let mut ascii = [b'z'; LEN*2];
        for (i, octet) in self.0.iter().enumerate() {
            ascii[i*2] = HEX_CHARS[(octet >> 4) as usize];
            ascii[i*2+1] = HEX_CHARS[(octet & 0x0f) as usize];
        }
        ascii
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Signature([0; LEN])
    }

    #[cfg(test)]
    pub fn sample() -> Self {
        let mut bytes = [0x88; LEN];
        bytes[1] = 0x4a;
        bytes[12] = 0x9c;
        bytes[50] = 0x00;
        Signature(bytes)
    }
}

impl Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Signature({})", self)
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let ascii = self.hex_bytes();
        let hex = unsafe { str::from_utf8_unchecked(&ascii) };
        f.write_str(&hex)
    }
}

#[test]
fn hex() {
    let digest = Signature([0xff; LEN]);
    let hex = format!("{}", digest);
    assert_eq!(hex.len(), 128);
    for b in hex.bytes() {
        assert_eq!(b, b'f');
    }
}

#[test]
fn eq() {
    let x = Signature::sample();
    let z = Signature::zero();
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

    let orig = Signature::sample();
    let there_and_back_again = Bincoded::new(&orig).unwrap().deserialize().unwrap();
    assert_eq!(orig, there_and_back_again);
}

#[test]
fn bincoded_repr() {
    use super::Bincoded;

    let orig = Signature::sample();
    let coded = Bincoded::new(&orig).unwrap();
    assert_eq!(coded.as_ref().len(), LEN);
    assert_eq!(&orig.0[..], coded.as_ref());
}
