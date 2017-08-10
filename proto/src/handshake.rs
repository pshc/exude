//! Messages sent client--server. (before driver is loaded)

use std::borrow::Borrow;

use super::Digest;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DriverInfo {
    pub len: usize,
    pub digest: super::Digest,
    pub sig: super::Signature,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Welcome<M: Borrow<DriverInfo> = Box<DriverInfo>> {
    Current,
    Obsolete,
    Download(String, M),
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Hello {
    Newbie,
    Cached(Digest),
    Oneshot(Digest),
}

/// Control messages from driver to loader.
#[derive(Debug, Deserialize, Serialize)]
pub enum UpControl {
    Download(String, Box<DriverInfo>),
}
