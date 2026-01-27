use crate::transport::client::{ClientEvent, ClientTransport};
use crate::transport::common::Channel;
use crate::transport::error::TransportError;
use godot::builtin::{
    Dictionary, GString, PackedByteArray, PackedStringArray, Variant, varray, vdict,
};
use godot::classes::{
    HttpClient, Json, Node, Object, WebRtcDataChannel, WebRtcPeerConnection,
    http_client::Method, http_client::Status,
};
use godot::global::{Error, godot_print};
use godot::meta::ToGodot;
use godot::obj::{Base, Gd, NewAlloc, NewGd};
use godot::prelude::{GodotClass, INode, godot_api};

const DATA_CHANNEL_NAME: &str = "game";

#[derive(Debug, Clone)]
struct SdpDescription {
    type_: GString,
    sdp: GString,
}

/// Minimal node for WebRTC signals (required by Godot)
/// Stores the offer when the signal fires, transport can retrieve it
#[derive(GodotClass)]
#[class(base=Node)]
struct SignalNode {
    stored_offer_type: Option<GString>,
    stored_offer_sdp: Option<GString>,
    #[base]
    base: Base<Node>,
}

#[godot_api]
impl INode for SignalNode {
    fn init(base: Base<Node>) -> Self {
        Self {
            stored_offer_type: None,
            stored_offer_sdp: None,
            base,
        }
    }
}

#[godot_api]
impl SignalNode {
    #[func]
    fn _on_session_description_created(&mut self, type_: GString, sdp: GString) {
        godot_print!("[WebRTC] Signal fired: {} - {}", type_, sdp);
        self.stored_offer_type = Some(type_);
        self.stored_offer_sdp = Some(sdp);
    }

    #[func]
    fn take_offer(&mut self) -> Variant {
        if let (Some(type_), Some(sdp)) = (self.stored_offer_type.take(), self.stored_offer_sdp.take()) {
            let dict = vdict! {
                "type": type_,
                "sdp": sdp,
            };
            dict.to_variant()
        } else {
            Variant::nil()
        }
    }
}

/// WebRTC client transport implementation
/// Handles WebRTC peer connection and HTTP signaling
pub struct WebRTCClientTransport {
    peer_connection: Gd<WebRtcPeerConnection>,
    data_channel: Gd<WebRtcDataChannel>,
    http_client: Gd<HttpClient>,
    signal_node: Gd<SignalNode>,
    signaling_url: String,
    connected: bool,
    // Signaling state
    pending_offer: Option<SdpDescription>,
    pending_request_body: Option<PackedByteArray>,
    signaling_complete: bool,
    offer_created: bool,
}

impl WebRTCClientTransport {
    /// Create a new WebRTC client transport
    /// Performs HTTP signaling roundtrip: create offer -> POST -> receive answer -> setup connection
    pub fn new(signaling_url: String, parent_node: Gd<Node>) -> Result<Self, TransportError> {
        godot_print!("[WebRTC] Creating transport with signaling URL: {}", signaling_url);

        // Create peer connection
        let mut peer_connection = WebRtcPeerConnection::new_gd();
        let ice_servers = vdict! {
            "iceServers": varray![vdict! {
                "urls": varray!["stun:stun.l.google.com:19302"],
            }],
        };

        match peer_connection
            .initialize_ex()
            .configuration(&ice_servers)
            .done()
        {
            Error::OK => {}
            err => {
                return Err(TransportError::Other(format!(
                    "Failed to initialize WebRTC peer connection: {:?}",
                    err
                )));
            }
        }

        // Create data channel
        let data_channel = peer_connection
            .create_data_channel(DATA_CHANNEL_NAME)
            .ok_or_else(|| TransportError::Other("Failed to create data channel".to_string()))?;

        // Create signal node and add to parent
        let mut signal_node = SignalNode::new_alloc();
        signal_node.set_name("WebRTCSignalNode");
        let mut parent_obj = parent_node.upcast::<Object>();
        let add_child_name = godot::builtin::StringName::from("add_child");
        let _ = parent_obj.call(
            &add_child_name,
            &[signal_node.clone().upcast::<Object>().to_variant()],
        );

        // Create HTTP client
        let http_client = HttpClient::new_gd();

        // Create transport (we'll set up the callback after)
        let mut transport = Self {
            peer_connection: peer_connection.clone(),
            data_channel,
            http_client,
            signal_node: signal_node.clone(),
            signaling_url: signaling_url.clone(),
            connected: false,
            pending_offer: None,
            pending_request_body: None,
            signaling_complete: false,
            offer_created: false,
        };

        // Connect WebRTC signal to signal node
        let mut pc_obj = peer_connection.clone().upcast::<Object>();
        let node_obj = signal_node.clone().upcast::<Object>();
        let sdp_callable = node_obj.callable("_on_session_description_created");
        match pc_obj.connect("session_description_created", &sdp_callable) {
            Error::OK => {}
            err => {
                return Err(TransportError::Other(format!("Failed to connect signal: {:?}", err)));
            }
        }

        // Start signaling: create offer
        godot_print!("[WebRTC] Creating offer...");
        match transport.peer_connection.create_offer() {
            Error::OK => {
                transport.offer_created = true;
            }
            err => {
                return Err(TransportError::Other(format!("Failed to create offer: {:?}", err)));
            }
        }

        Ok(transport)
    }


    /// Poll WebRTC connection and handle signaling
    /// Must be called regularly (every frame) for HTTP client to progress
    pub fn poll(&mut self) -> Result<(), TransportError> {
        // Poll peer connection (processes WebRTC events and signals)
        self.peer_connection.poll();

        // Check if signal node has a stored offer
        if self.pending_offer.is_none() && !self.signaling_complete {
            let mut node_obj = self.signal_node.clone().upcast::<Object>();
            let take_offer_name = godot::builtin::StringName::from("take_offer");
            let offer_variant: Variant = node_obj.call(&take_offer_name, &[]);
            if !offer_variant.is_nil() {
                if let Ok(offer_dict) = offer_variant.try_to::<Dictionary>() {
                    if let (Some(type_), Some(sdp)) = (
                        offer_dict.get("type").and_then(|v: Variant| v.try_to::<GString>().ok()),
                        offer_dict.get("sdp").and_then(|v: Variant| v.try_to::<GString>().ok()),
                    ) {
                        godot_print!("[WebRTC] Got offer from signal node: {}", type_);
                        self.pending_offer = Some(SdpDescription { type_, sdp });
                    }
                }
            }
        }

        // Handle signaling if not complete (this polls HTTP client)
        if !self.signaling_complete {
            self.process_signaling()?;
        }

        // Update connection status
        self.update_connection_status();

        Ok(())
    }

    /// Process signaling: send offer, receive answer
    fn process_signaling(&mut self) -> Result<(), TransportError> {
        // Step 1: If we have a pending offer, set local description and send to server
        if let Some(offer) = self.pending_offer.take() {
            godot_print!("[WebRTC] Setting local description and sending offer");
            match self
                .peer_connection
                .set_local_description(&offer.type_, &offer.sdp)
            {
                Error::OK => {
                    self.send_offer_to_server(&offer)?;
                }
                err => {
                    return Err(TransportError::Other(format!(
                        "Failed to set local description: {:?}",
                        err
                    )));
                }
            }
        }

        // Step 2: Process HTTP client to receive answer
        // Poll HTTP client to advance connection state
        self.http_client.poll();
        let status = self.http_client.get_status();

        match status {
            Status::RESOLVING => {
                // DNS resolution in progress - keep polling
                godot_print!("[WebRTC] HTTP client status: RESOLVING (polling...)");
            }
            Status::CONNECTING => {
                // TCP connection in progress - keep polling
                godot_print!("[WebRTC] HTTP client status: CONNECTING (polling...)");
            }
            Status::CONNECTED => {
                godot_print!("[WebRTC] HTTP client status: CONNECTED");
                // Send request if we have pending request body
                if let Some(body) = self.pending_request_body.take() {
                    godot_print!("[WebRTC] Sending offer to signaling server (body size: {} bytes)", body.len());
                    let mut headers = PackedStringArray::new();
                    headers.push("Content-Type: application/json");
                    // Use request_raw with full URL (works for both HTTP and HTTPS)
                    match self.http_client.request_raw(Method::POST, &self.signaling_url, &headers, &body) {
                        Error::OK => {
                            godot_print!("[WebRTC] HTTP POST request sent successfully");
                        }
                        err => {
                            return Err(TransportError::Other(format!("Failed to send request: {:?}", err)));
                        }
                    }
                } else {
                    godot_print!("[WebRTC] HTTP client connected but no pending request body");
                }
            }
            Status::REQUESTING => {
                godot_print!("[WebRTC] HTTP client status: REQUESTING");
            }
            Status::BODY => {
                godot_print!("[WebRTC] HTTP client status: BODY (reading response)");
                // Read response
                let mut body = PackedByteArray::new();
                loop {
                    self.http_client.poll();
                    let chunk = self.http_client.read_response_body_chunk();
                    if chunk.len() == 0 {
                        break;
                    }
                    body.extend_array(&chunk);
                }

                let response_code = self.http_client.get_response_code();
                godot_print!("[WebRTC] HTTP response code: {}", response_code);
                if response_code == 200 {
                    godot_print!("[WebRTC] Received answer, setting remote description");
                    self.handle_answer_response(body)?;
                    self.signaling_complete = true;
                } else {
                    return Err(TransportError::Other(format!(
                        "Signaling failed with code: {}",
                        response_code
                    )));
                }
            }
            Status::DISCONNECTED => {
                godot_print!("[WebRTC] HTTP client status: DISCONNECTED");
            }
            _ => {
                godot_print!("[WebRTC] HTTP client status: {:?}", status);
            }
        }

        Ok(())
    }

    /// Send offer to signaling server via HTTP POST
    fn send_offer_to_server(&mut self, offer: &SdpDescription) -> Result<(), TransportError> {
        // Create JSON body
        let offer_dict = vdict! {
            "type": offer.type_.clone(),
            "sdp": offer.sdp.clone(),
        };
        let json_str = Json::stringify(&offer_dict.to_variant());
        let json_bytes = PackedByteArray::from(json_str.to_string().as_bytes());

        // Store request body - we'll send it when HTTP client is ready
        self.pending_request_body = Some(json_bytes);
        
        // Parse URL to get host for connection
        let (host, port, _path) = Self::parse_url(&self.signaling_url)?;
        let is_https = self.signaling_url.starts_with("https://");
        
        // For HttpClient, we connect to host:port, then use request_raw with full URL
        let host_with_port = if port == 80 || port == 443 {
            host.clone()
        } else {
            format!("{}:{}", host, port)
        };

        godot_print!("[WebRTC] Initiating connection to: {} (port: {}, https: {})", host_with_port, port, is_https);
        
        // Connect to host (HttpClient handles HTTPS/TLS automatically)
        // Note: For HTTPS, HttpClient will handle TLS handshake after connection
        match self.http_client.connect_to_host(&host_with_port) {
            Error::OK => {
                let initial_status = self.http_client.get_status();
                godot_print!("[WebRTC] connect_to_host() succeeded, initial status: {:?}", initial_status);
                // Status will be RESOLVING or CONNECTING - we need to poll to progress
            }
            err => {
                return Err(TransportError::Other(format!("Failed to connect: {:?}", err)));
            }
        }

        Ok(())
    }

    /// Handle answer response from server
    fn handle_answer_response(&mut self, body: PackedByteArray) -> Result<(), TransportError> {
        let body_str = String::from_utf8(body.to_vec())
            .map_err(|e| TransportError::Other(format!("Invalid UTF-8: {}", e)))?;

        let json_result = Json::parse_string(&body_str);
        let answer_dict = json_result
            .try_to::<Dictionary>()
            .map_err(|_| TransportError::Other("Invalid JSON response".to_string()))?;

        let type_ = answer_dict
            .get("type")
            .ok_or_else(|| TransportError::Other("Missing 'type' in answer".to_string()))?
            .try_to::<GString>()
            .map_err(|_| TransportError::Other("Invalid 'type' field".to_string()))?;

        let sdp = answer_dict
            .get("sdp")
            .ok_or_else(|| TransportError::Other("Missing 'sdp' in answer".to_string()))?
            .try_to::<GString>()
            .map_err(|_| TransportError::Other("Invalid 'sdp' field".to_string()))?;

        match self.peer_connection.set_remote_description(&type_, &sdp) {
            Error::OK => {
                godot_print!("[WebRTC] Remote description set, signaling complete");
                Ok(())
            }
            err => Err(TransportError::Other(format!(
                "Failed to set remote description: {:?}",
                err
            ))),
        }
    }

    /// Parse URL into host, port, and path
    fn parse_url(url_str: &str) -> Result<(String, i32, String), TransportError> {
        let url_str = url_str.trim();
        let (scheme, rest) = if url_str.starts_with("https://") {
            ("https", &url_str[8..])
        } else if url_str.starts_with("http://") {
            ("http", &url_str[7..])
        } else {
            return Err(TransportError::Other("URL must start with http:// or https://".to_string()));
        };

        let default_port = if scheme == "https" { 443 } else { 80 };

        let (host_port, path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, "/"),
        };

        let (host, port) = match host_port.find(':') {
            Some(pos) => {
                let host = host_port[..pos].to_string();
                let port = host_port[pos + 1..]
                    .parse::<i32>()
                    .map_err(|_| TransportError::Other("Invalid port".to_string()))?;
                (host, port)
            }
            None => (host_port.to_string(), default_port),
        };

        Ok((host, port, path.to_string()))
    }

    /// Update connection status
    fn update_connection_status(&mut self) {
        use godot::classes::web_rtc_peer_connection::ConnectionState;
        use godot::classes::web_rtc_data_channel::ChannelState;

        let pc_state = self.peer_connection.get_connection_state();
        let channel_state = self.data_channel.get_ready_state();
        self.connected = pc_state == ConnectionState::CONNECTED
            && channel_state == ChannelState::OPEN
            && self.signaling_complete;
    }

    fn is_channel_ready(&self) -> bool {
        use godot::classes::web_rtc_data_channel::ChannelState;
        self.data_channel.get_ready_state() == ChannelState::OPEN
    }
}


impl ClientTransport for WebRTCClientTransport {
    fn send(&mut self, data: Vec<u8>, _channel: Channel) -> Result<(), TransportError> {
        if !self.is_channel_ready() {
            return Err(TransportError::Other("Data channel not ready".to_string()));
        }

        let packet = PackedByteArray::from(data.as_slice());
        match self.data_channel.put_packet(&packet) {
            Error::OK => Ok(()),
            err => Err(TransportError::Other(format!("Failed to send: {:?}", err))),
        }
    }

    fn recv(&mut self) -> Result<Vec<ClientEvent>, TransportError> {
        // Poll to process signaling and connection updates
        self.poll()?;

        let mut events = Vec::new();
        while self.data_channel.get_available_packet_count() > 0 {
            let packet = self.data_channel.get_packet();
            events.push(ClientEvent::PacketReceived {
                data: packet.to_vec(),
                channel: Channel::Reliable,
            });
        }

        Ok(events)
    }

    fn is_connected(&self) -> bool {
        self.connected && self.is_channel_ready()
    }

    fn send_keepalive(&mut self) -> Result<(), TransportError> {
        self.poll()
    }
}
