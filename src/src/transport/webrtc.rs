use crate::transport::client::{ClientEvent, ClientTransport};
use crate::transport::common::Channel;
use crate::transport::error::TransportError;
use godot::builtin::{
    Dictionary, GString, PackedByteArray, PackedStringArray, StringName, Variant, varray, vdict,
};
use godot::builtin::Callable;
use godot::classes::{
    HttpClient, Json, Node, Object, WebRtcDataChannel, WebRtcPeerConnection, http_client::Method, http_client::Status,
};
use godot::global::{Error, godot_print};
use godot::meta::ToGodot;
use godot::obj::{Base, Gd, NewAlloc, NewGd, WithBaseField};
use godot::prelude::{GodotClass, INode, godot_api};

// TODO(@nohehf): Split this into multiple files, extract signaling logic

/// WebRTC client transport implementation
/// Based on GDScript example: extends Node with WebRTCPeerConnection
/// See: https://github.com/godotengine/godot-demo-projects/blob/master/networking/webrtc_signaling/README.md
pub struct WebRTCClientTransport {
    /// WebRTC peer connection to the relay server
    peer_connection: Gd<WebRtcPeerConnection>,
    /// Data channel for communication
    data_channel: Gd<WebRtcDataChannel>,
    /// HTTP client for signaling (doesn't need scene tree)
    http_client: Gd<HttpClient>,
    /// Signal handler node (receives signals and calls poll)
    signal_handler: Gd<SignalHandler>,
    /// Pending offer received from signal (stored directly, no Rc<RefCell> needed)
    pending_offer: Option<SdpDescription>,
    /// Signaling server URL (e.g., "http://localhost:8080")
    signaling_url: String,
    /// Connection status
    connected: bool,
    /// Signaling state: waiting for offer, waiting for answer, etc.
    signaling_state: SignalingState,
    /// Pending HTTP request body (offer JSON) to send when connected
    pending_http_request_body: Option<PackedByteArray>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SignalingState {
    NotStarted,
    WaitingForOffer,
    WaitingForAnswer,
    Complete,
}

const DATA_CHANNEL_NAME: &str = "game";

/// Minimal node ONLY for receiving WebRTC signals and calling poll (unavoidable - Godot requires nodes)
#[derive(GodotClass)]
#[class(base=Node)]
struct SignalHandler {
    poll_callable: Callable,
    offer_callback: Callable, // Callback to store offer in transport
    #[base]
    base: Base<Node>,
}

#[godot_api]
impl INode for SignalHandler {
    fn init(base: Base<Node>) -> Self {
        Self {
            poll_callable: Callable::invalid(),
            offer_callback: Callable::invalid(),
            base,
        }
    }

    fn ready(&mut self) {
        // Enable processing to call poll every frame
        self.base_mut().set_process(true);
    }

    fn process(&mut self, _delta: f64) {
        // Call poll callback if valid (calls WebNodeTunnelPeer::poll() which calls transport.poll())
        if self.poll_callable.is_valid() {
            let _ = self.poll_callable.call(&[]);
        }
    }
}

#[godot_api]
impl SignalHandler {
    #[func]
    fn on_session_description_created(&mut self, type_: GString, sdp: GString) {
        // Defer callback call to avoid bind conflicts - idiomatic Godot!
        if self.offer_callback.is_valid() {
            // Use call_deferred to release borrow before callback executes
            let callback = self.offer_callback.clone();
            let type_var = type_.to_variant();
            let sdp_var = sdp.to_variant();
            self.base_mut().call_deferred("_call_offer_callback", &[callback.to_variant(), type_var, sdp_var]);
        }
    }
    
    #[func]
    fn _call_offer_callback(&mut self, callback: Variant, type_: Variant, sdp: Variant) {
        // This is called deferred, so no bind conflicts!
        if let Ok(callable) = callback.try_to::<Callable>() {
            let _ = callable.call(&[type_, sdp]);
        }
    }

    #[func]
    fn set_poll_callable(&mut self, callable: Callable) {
        self.poll_callable = callable;
    }
    
    #[func]
    fn set_offer_callback(&mut self, callable: Callable) {
        self.offer_callback = callable;
    }
}

/// Represents an SDP (Session Description Protocol) description
#[derive(Debug, Clone)]
struct SdpDescription {
    type_: GString,
    sdp: GString,
}


impl WebRTCClientTransport {
    /// Create a new WebRTC client transport
    /// Initializes peer connection with ICE servers, creates a data channel, and sets up HTTP signaling
    ///
    /// # Arguments
    /// * `signaling_url` - HTTP URL for signaling server (e.g., "http://localhost:8080")
    /// * `signal_node` - A node in the scene tree (only needed for WebRTC signals - unavoidable)
    /// * `poll_callable` - Callable to call poll() on the peer (for automatic polling)
    /// * `offer_callback` - Callable to call when offer is received (stores it in transport)
    pub fn new(signaling_url: String, signal_node: Gd<Node>, poll_callable: Callable, offer_callback: Callable) -> Result<Self, TransportError> {
        godot_print!(
            "Creating WebRTC client transport with signaling URL: {}",
            signaling_url
        );
        let mut peer_connection = WebRtcPeerConnection::new_gd();

        // Initialize with ICE servers (same as GDScript: pc.initialize({"iceServers": [...]}))
        let ice_servers = vdict! {
            "iceServers": vec![vdict! {
                "urls": varray!["stun:stun.l.google.com:19302"],
            }],
        };

        godot_print!("Initializing WebRTC peer connection");
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

        godot_print!("Creating data channel");
        // Create data channel (same as GDScript: channel = pc.create_data_channel("game"))
        let data_channel = peer_connection
            .create_data_channel(DATA_CHANNEL_NAME)
            .ok_or_else(|| TransportError::Other("Failed to create data channel".to_string()))?;

        godot_print!("Creating HTTP client");
        // Create HTTP client for signaling (doesn't need scene tree!)
        let http_client = HttpClient::new_gd();

        // Create minimal signal handler (ONLY for WebRTC signals and polling - unavoidable)
        let mut signal_handler = SignalHandler::new_alloc();
        {
            let mut handler = signal_handler.bind_mut();
            handler.set_poll_callable(poll_callable);
            handler.set_offer_callback(offer_callback);
        }
        
        // Add to provided node (must be in scene tree for signals to work)
        let mut signal_node_obj = signal_node.upcast::<Object>();
        let add_child_name = StringName::from("add_child");
        let handler_variant = signal_handler.clone().upcast::<Object>().to_variant();
        let _ = signal_node_obj.call(&add_child_name, &[handler_variant]);

        let mut transport = Self {
            peer_connection: peer_connection.clone(),
            data_channel,
            http_client: http_client.clone(),
            signal_handler: signal_handler.clone(),
            pending_offer: None,
            signaling_url,
            connected: false,
            signaling_state: SignalingState::NotStarted,
            pending_http_request_body: None,
        };
        
        // Set up offer callback - create callable to a method that stores offer in transport
        // We'll use a method on WebNodeTunnelPeer that can access the transport
        // For now, we'll check for offers in poll() using get_pending_offer() which doesn't require mut

        // Connect WebRTC signal to handler
        let mut pc_obj = transport.peer_connection.clone().upcast::<Object>();
        let handler_obj = signal_handler.clone().upcast::<Object>();
        let sdp_callable = handler_obj.callable("on_session_description_created");

        match pc_obj.connect("session_description_created", &sdp_callable) {
            Error::OK => {}
            err => {
                return Err(TransportError::Other(format!(
                    "Failed to connect session_description_created signal: {:?}",
                    err
                )));
            }
        }
        

        godot_print!("Starting signaling process");
        // Start signaling process
        transport.start_signaling()?;

        Ok(transport)
    }

    /// Set pending offer (called from signal handler callback)
    pub fn set_pending_offer(&mut self, type_: GString, sdp: GString) {
        self.pending_offer = Some(SdpDescription { type_, sdp });
    }
    
    /// Start signaling process - creates offer and sends to server
    fn start_signaling(&mut self) -> Result<(), TransportError> {
        self.signaling_state = SignalingState::WaitingForOffer;
        self.pending_offer = None; // Clear any pending offer

        godot_print!("Creating WebRTC offer...");
        // Create offer (triggers session_description_created signal)
        match self.peer_connection.create_offer() {
            Error::OK => {
                godot_print!("WebRTC offer creation initiated");
                Ok(())
            }
            err => Err(TransportError::Other(format!(
                "Failed to create offer: {:?}",
                err
            ))),
        }
    }

    /// Parse URL into host, port, and path
    fn parse_url(url_str: &str) -> Result<(String, i32, String), TransportError> {
        // Simple URL parsing: http://host:port/path or https://host:port/path
        let url_str = url_str.trim();
        let (scheme, rest) = if url_str.starts_with("https://") {
            ("https", &url_str[8..])
        } else if url_str.starts_with("http://") {
            ("http", &url_str[7..])
        } else {
            return Err(TransportError::Other("URL must start with http:// or https://".to_string()));
        };
        
        let default_port = if scheme == "https" { 443 } else { 80 };
        
        // Split host:port from path
        let (host_port, path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, "/"),
        };
        
        // Split host and port
        let (host, port) = match host_port.find(':') {
            Some(pos) => {
                let host = host_port[..pos].to_string();
                let port_str = &host_port[pos + 1..];
                let port = port_str.parse::<i32>()
                    .map_err(|_| TransportError::Other("Invalid port number".to_string()))?;
                (host, port)
            }
            None => (host_port.to_string(), default_port),
        };
        
        Ok((host, port, path.to_string()))
    }
    
    /// Send offer to signaling server via HTTP POST using HTTPClient
    fn send_offer_to_server(&mut self, offer: SdpDescription) -> Result<(), TransportError> {
        godot_print!("Preparing to send offer to server: {}", self.signaling_url);
        
        // Create JSON body and store it
        let json_bytes = Self::create_offer_json(&offer)?;
        godot_print!("Created JSON body, size: {} bytes", json_bytes.len());
        self.pending_http_request_body = Some(json_bytes);
        
        // Parse URL - HttpClient.connect_to_host handles host:port format
        let (host, port, _path) = Self::parse_url(&self.signaling_url)?;
        let host_with_port = if port == 80 || port == 443 {
            host.clone()
        } else {
            format!("{}:{}", host, port)
        };
        
        // Connect to host (will be processed in poll())
        // Note: HttpClient.connect_to_host doesn't handle SSL - we'll need to handle that differently
        // For now, assume HTTPS URLs will work if the server supports it
        match self.http_client.connect_to_host(&host_with_port) {
            Error::OK => {
                godot_print!("Initiating connection to {}...", host_with_port);
                Ok(())
            }
            err => Err(TransportError::Other(format!("Failed to connect to host: {:?}", err))),
        }
    }
    
    /// Process HTTP client - handles connection, request sending, and response reading
    fn process_http_client(&mut self) -> Result<Option<(i64, PackedByteArray)>, TransportError> {
        self.http_client.poll(); // Always poll HTTP client
        
        let status = self.http_client.get_status();
        
        match status {
            Status::CONNECTING | Status::RESOLVING => {
                // Still connecting, wait
                Ok(None)
            }
            Status::CONNECTED => {
                // Connected, send the request with body
                if let Some(body) = self.pending_http_request_body.take() {
                    godot_print!("HTTP client connected, sending request with body ({} bytes)", body.len());
                    
                    let mut headers = PackedStringArray::new();
                    headers.push("Content-Type: application/json");
                    
                    // Use request_raw which accepts the body directly
                    match self.http_client.request_raw(Method::POST, &self.signaling_url, &headers, &body) {
                        Error::OK => {
                            godot_print!("HTTP POST request sent successfully with body");
                        }
                        err => {
                            return Err(TransportError::Other(format!("Failed to send HTTP request: {:?}", err)));
                        }
                    }
                }
                Ok(None)
            }
            Status::REQUESTING => {
                // Request sent, waiting for response
                Ok(None)
            }
            Status::BODY => {
                // Response received, read body
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
                godot_print!("HTTP response received: {}", response_code);
                self.http_client.close();
                Ok(Some((response_code as i64, body)))
            }
            Status::DISCONNECTED => {
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Poll the peer connection and handle signaling state
    /// Should be called regularly (e.g., in _process)
    pub fn poll(&mut self) -> Result<(), TransportError> {
        // Poll the peer connection
        match self.peer_connection.poll() {
            Error::OK => {}
            err => {
                return Err(TransportError::Other(format!("Poll error: {:?}", err)));
            }
        }

        // Process HTTP client (connection, request, response)
        let http_result = self.process_http_client();
        if let Ok(Some((response_code, body))) = http_result {
            if self.signaling_state == SignalingState::WaitingForAnswer {
                if response_code == 200 {
                    godot_print!("Handling answer response");
                    self.handle_answer_response(body)?;
                } else {
                    return Err(TransportError::Other(format!(
                        "Signaling failed: {}",
                        response_code
                    )));
                }
            }
        }

        // Check if offer was created and send it via HTTP
        if self.signaling_state == SignalingState::WaitingForOffer {
            // Check pending offer (set directly by signal handler callback - no bind conflicts!)
            if let Some(offer) = self.pending_offer.take() {
                godot_print!("Offer received from signal, type: {}", offer.type_);
                // Set local description first
                match self
                    .peer_connection
                    .set_local_description(&offer.type_, &offer.sdp)
                {
                    Error::OK => {
                        godot_print!("Local description set, sending offer to server");
                        // Send offer to server via HTTP
                        self.send_offer_to_server(offer)?;
                        self.signaling_state = SignalingState::WaitingForAnswer;
                    }
                    err => {
                        return Err(TransportError::Other(format!(
                            "Failed to set local description: {:?}",
                            err
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle the answer response from HTTP request
    fn handle_answer_response(&mut self, body: PackedByteArray) -> Result<(), TransportError> {
        let answer = Self::parse_answer_json(body)?;

        // Set remote description
        match self
            .peer_connection
            .set_remote_description(&answer.type_, &answer.sdp)
        {
            Error::OK => {
                self.signaling_state = SignalingState::Complete;
                Ok(())
            }
            err => Err(TransportError::Other(format!(
                "Failed to set remote description: {:?}",
                err
            ))),
        }
    }

    // ============================================================================
    // Signaling parsing helpers
    // ============================================================================

    /// Parse answer from HTTP response JSON body
    fn parse_answer_json(body: PackedByteArray) -> Result<SdpDescription, TransportError> {
        // Convert body to string
        let body_str = String::from_utf8(body.to_vec())
            .map_err(|e| TransportError::Other(format!("Failed to parse response body: {}", e)))?;

        // Parse JSON
        let json_result = Json::parse_string(&body_str);
        let answer_dict = json_result.try_to::<Dictionary>().map_err(|_| {
            TransportError::Other("Failed to parse answer as Dictionary".to_string())
        })?;

        Self::extract_sdp_from_dict(&answer_dict)
    }

    /// Extract SDP description (type and sdp) from a Dictionary
    fn extract_sdp_from_dict(dict: &Dictionary) -> Result<SdpDescription, TransportError> {
        let type_ = Self::get_string_field(dict, "type")?;
        let sdp = Self::get_string_field(dict, "sdp")?;

        Ok(SdpDescription { type_, sdp })
    }

    /// Get a string field from a Dictionary, returning a descriptive error if missing or invalid
    fn get_string_field(dict: &Dictionary, field_name: &str) -> Result<GString, TransportError> {
        dict.get(field_name)
            .ok_or_else(|| TransportError::Other(format!("Missing '{}' in JSON", field_name)))
            .and_then(|v| {
                v.try_to::<GString>().map_err(|_| {
                    TransportError::Other(format!("Failed to get '{}' from JSON", field_name))
                })
            })
    }

    /// Create JSON bytes for an offer/answer to send via HTTP
    fn create_offer_json(offer: &SdpDescription) -> Result<PackedByteArray, TransportError> {
        let offer_dict = vdict! {
            "type": offer.type_.clone(),
            "sdp": offer.sdp.clone(),
        };
        let json_body = Json::stringify(&offer_dict.to_variant());
        Ok(PackedByteArray::from(json_body.to_string().as_bytes()))
    }

    /// Check if data channel is ready/open
    fn is_channel_ready(&self) -> bool {
        use godot::classes::web_rtc_data_channel::ChannelState;
        self.data_channel.get_ready_state() == ChannelState::OPEN
    }

    /// Update connection status based on peer connection state
    fn update_connection_status(&mut self) {
        use godot::classes::web_rtc_peer_connection::ConnectionState;
        let state = self.peer_connection.get_connection_state();
        self.connected = state == ConnectionState::CONNECTED && self.is_channel_ready();
    }
}

impl ClientTransport for WebRTCClientTransport {
    /// Send data through the WebRTC data channel
    /// Same as GDScript: channel.put_packet(data)
    fn send(&mut self, data: Vec<u8>, _channel: Channel) -> Result<(), TransportError> {
        if !self.is_channel_ready() {
            return Err(TransportError::Other(
                "Data channel is not ready".to_string(),
            ));
        }

        let packet = PackedByteArray::from(data.as_slice());
        match self.data_channel.put_packet(&packet) {
            Error::OK => Ok(()),
            err => Err(TransportError::Other(format!(
                "Failed to send packet: {:?}",
                err
            ))),
        }
    }

    /// Receive data from the WebRTC data channel
    /// Same as GDScript: channel.get_packet()
    fn recv(&mut self) -> Result<Vec<ClientEvent>, TransportError> {
        // Poll first to process any pending messages
        self.poll()?;

        let mut events = Vec::new();

        // Check for available packets (same as GDScript: channel.get_packet())
        while self.data_channel.get_available_packet_count() > 0 {
            let packet = self.data_channel.get_packet();
            let data = packet.to_vec();
            events.push(ClientEvent::PacketReceived {
                data,
                channel: Channel::Reliable, // WebRTC data channels are reliable by default
            });
        }

        // Update connection status after receiving
        self.update_connection_status();

        Ok(events)
    }

    fn is_connected(&self) -> bool {
        self.connected && self.is_channel_ready()
    }

    fn send_keepalive(&mut self) -> Result<(), TransportError> {
        // Use WebRTC polling as keepalive - polling maintains the connection
        // and processes ICE candidates, which acts as a keepalive mechanism
        self.poll()?;

        // Also update connection status to ensure we're tracking state correctly
        self.update_connection_status();

        Ok(())
    }
}
