use super::DriverInfo;

#[derive(Debug, Deserialize, Serialize)]
pub enum UpRequest {
    Ping(u32),
    Bye,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum DownResponse {
    ProposeUpgrade(Box<DriverInfo>),
    Pong(u32),
}
