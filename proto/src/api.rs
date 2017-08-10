use super::DriverInfo;

#[derive(Debug, Deserialize, Serialize)]
pub enum UpRequest {
    Ping(u32),
    Bye,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum DownResponse {
    ProposeUpgrade(String, Box<DriverInfo>),
    Pong(u32),
    Goats(u32),
}
