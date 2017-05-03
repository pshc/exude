//! Messages sent client--server. (before driver is loaded)

use super::Digest;

/// Maximum byte length of an InlineDriver payload.
pub static INLINE_MAX: usize = 100_000_000;


#[derive(Debug, Deserialize, Serialize)]
pub struct DriverInfo {
    pub len: usize,
    pub digest: super::Digest,
    pub sig: super::Signature,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Welcome {
    Current,
    InlineDriver(DriverInfo),
    DownloadDriver(String, DriverInfo),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Hello(pub Option<Digest>);
