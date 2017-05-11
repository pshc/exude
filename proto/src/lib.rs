pub extern crate bincode;
pub extern crate serde;
#[macro_use]
extern crate serde_derive;

pub mod api;
pub mod bincoded;
pub mod digest;
pub mod handshake;
pub mod sig;

pub use self::bincoded::Bincoded;
pub use self::digest::Digest;
pub use self::handshake::DriverInfo;
pub use self::sig::Signature;

static HEX_CHARS: &[u8] = b"0123456789abcdef";
