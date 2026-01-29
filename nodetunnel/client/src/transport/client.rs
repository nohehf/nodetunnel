use crate::transport::common::Channel;
use crate::transport::error::TransportError;

#[derive(Debug, Clone)]
pub enum ClientEvent {
    PacketReceived { data: Vec<u8>, channel: Channel },
}

pub trait ClientTransport {
    fn send(&mut self, data: Vec<u8>, channel: Channel) -> Result<(), TransportError>;
    fn recv(&mut self) -> Result<Vec<ClientEvent>, TransportError>;
    fn is_connected(&self) -> bool;
    fn send_keepalive(&mut self) -> Result<(), TransportError>;
}
