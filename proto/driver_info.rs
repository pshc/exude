#[derive(Debug, Deserialize, Serialize)]
pub struct DriverInfo {
    pub len: usize,
    pub digest: super::Digest,
    pub sig: super::Signature,
}
