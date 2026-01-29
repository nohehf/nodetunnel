use super::common::{ServerEvent, TransferChannel};
use crate::transport::r#trait::Transport;
use crate::udp::error::UdpError;
use crate::udp::sessions::ConnectionManager;
use paperudp::channel::DecodeResult;
use paperudp::packet::PacketType;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{debug, info, trace, warn};

pub struct PaperInterface {
    pub(crate) socket: UdpSocket,
    pub(crate) connection_manager: ConnectionManager,
    pending_events: Vec<ServerEvent>,
}

impl PaperInterface {
    pub async fn new(addr: SocketAddr) -> Result<Self, UdpError> {
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| UdpError::BindError(e))?;

        let span = tracing::span!(tracing::Level::INFO, "transport", transport = "UDP");
        let _enter = span.enter();
        info!("Server bound to {}", addr);
        info!("To connect via UDP, send packets to {}", addr);

        Ok(Self {
            socket,
            connection_manager: ConnectionManager::new(),
            pending_events: Vec::new(),
        })
    }

    pub async fn recv_events(&mut self) -> Result<Vec<ServerEvent>, UdpError> {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "UDP");
        let _enter = span.enter();
        let mut buf = [0u8; 65535];

        loop {
            self.socket.readable().await.map_err(UdpError::RecvError)?;

            loop {
                match self.socket.try_recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        if len == 0 {
                            continue;
                        }

                        let (session_id, session_addr, res) = {
                            let (session, is_new) = self.connection_manager.get_or_create(addr);

                            if is_new {
                                info!(
                                    "New client connected from {} (client_id: {})",
                                    addr, session.id
                                );
                                self.pending_events.push(ServerEvent::ClientConnected {
                                    client_id: session.id,
                                })
                            }

                            session.last_heard_from = Instant::now();
                            let res = session.channel.decode(&buf[..len]);
                            (session.id, session.addr, res)
                        };

                        match res {
                            DecodeResult::Unreliable { payload } => {
                                for p in payload {
                                    if p == [3u8] {
                                        continue;
                                    }
                                    trace!(
                                        "Received unreliable packet from client {}: {} bytes",
                                        session_id,
                                        p.len()
                                    );
                                    self.pending_events.push(ServerEvent::PacketReceived {
                                        client_id: session_id,
                                        data: p,
                                        channel: TransferChannel::Unreliable,
                                    });
                                }
                            }
                            DecodeResult::Reliable {
                                payload,
                                ack_packet,
                                ..
                            } => {
                                for p in payload {
                                    trace!(
                                        "Received reliable packet from client {}: {} bytes",
                                        session_id,
                                        p.len()
                                    );
                                    self.pending_events.push(ServerEvent::PacketReceived {
                                        client_id: session_id,
                                        data: p,
                                        channel: TransferChannel::Reliable,
                                    });
                                }

                                if let Some(ack) = ack_packet {
                                    if let Err(e) =
                                        self.socket.send_to(ack.as_slice(), session_addr).await
                                    {
                                        warn!("Failed to send ack to {}: {}", session_addr, e);
                                    } else {
                                        trace!(
                                            "Sent ack to client {} at {}",
                                            session_id,
                                            session_addr
                                        );
                                    }
                                }
                            }
                            DecodeResult::Ack { .. } => {
                                trace!("Received ack from client {}", session_id);
                            }
                            DecodeResult::None => {
                                debug!(
                                    "Unknown packet from client {} at {}: {:?}",
                                    session_id,
                                    session_addr,
                                    &buf[..len]
                                );
                                self.remove_client(&session_id);
                            }
                        }
                    }

                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::Interrupted
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionRefused
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(UdpError::RecvError(e)),
                }
            }

            if !self.pending_events.is_empty() {
                return Ok(std::mem::take(&mut self.pending_events));
            }
        }
    }

    pub async fn do_resends(&mut self, interval: Duration) {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "UDP");
        let _enter = span.enter();
        for (addr, pkt) in self.connection_manager.get_resends(interval) {
            if let Err(e) = self.socket.send_to(&pkt, addr).await {
                warn!("Failed to resend packet to {}: {}", addr, e);
                continue;
            }
            trace!("Resent packet to {}", addr);
        }
    }
}

impl Transport for PaperInterface {
    async fn send(
        &mut self,
        client_id: u64,
        data: Vec<u8>,
        channel: TransferChannel,
    ) -> Result<(), std::io::Error> {
        let span = tracing::span!(tracing::Level::TRACE, "transport", transport = "UDP");
        let _enter = span.enter();
        if let Some(session) = self.connection_manager.get_by_id(&client_id) {
            match channel {
                TransferChannel::Reliable => {
                    let pkt = session.channel.encode(&*data, PacketType::ReliableOrdered);
                    trace!(
                        "Sending reliable packet to client {}: {} bytes",
                        client_id,
                        data.len()
                    );
                    self.socket.send_to(&pkt, session.addr).await?;
                }
                TransferChannel::Unreliable => {
                    let pkt = session.channel.encode(&data, PacketType::Unreliable);
                    trace!(
                        "Sending unreliable packet to client {}: {} bytes",
                        client_id,
                        data.len()
                    );
                    self.socket.send_to(&pkt, session.addr).await?;
                }
            }
        }
        Ok(())
    }

    fn remove_client(&mut self, client_id: &u64) {
        let span = tracing::span!(tracing::Level::DEBUG, "transport", transport = "UDP");
        let _enter = span.enter();
        debug!("Removing client {}", client_id);
        self.connection_manager.remove_session(client_id);
    }
}
