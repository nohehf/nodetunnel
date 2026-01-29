pub use nodetunnel_core::transport::channel::Channel as TransferChannel;

#[derive(Debug, Clone)]
pub enum ServerEvent {
    ClientConnected {
        client_id: u64,
    },
    ClientDisconnected {
        client_id: u64,
    },
    PacketReceived {
        client_id: u64,
        data: Vec<u8>,
        channel: TransferChannel,
    },
}
