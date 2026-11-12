//! A gateway is one socket at one literal address: a parameter in its path
//! would be read by nothing, so it is refused.

use nest_rs::ws::gateway;

#[gateway(path = "/ws/{room}")]
pub struct RoomGateway;

fn main() {}
