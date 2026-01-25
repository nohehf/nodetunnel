use crate::transport::client::{ClientEvent, ClientTransport};
use crate::transport::common::Channel;
use crate::transport::error::TransportError;
use godot::builtin::{Dictionary, GString, PackedByteArray, PackedStringArray, varray, vdict};
use godot::classes::{
    HttpRequest, Json, Node, Object, WebRtcDataChannel, WebRtcPeerConnection, http_client::Method,
};
use godot::global::Error;
use godot::meta::ToGodot;
use godot::obj::{Base, Gd, NewAlloc, NewGd};
use godot::prelude::{GodotClass, INode, godot_api};
use std::cell::RefCell;
use std::rc::Rc;

// TODO(@nohehf): Split this into multiple files, extract signaling logic

/// WebRTC client transport implementation
/// Based on GDScript example: extends Node with WebRTCPeerConnection
/// See: https://github.com/godotengine/godot-demo-projects/blob/master/networking/webrtc_signaling/README.md
pub struct WebRTCClientTransport {
    /// WebRTC peer connection to the relay server
    peer_connection: Gd<WebRtcPeerConnection>,
    /// Data channel for communication
    data_channel: Gd<WebRtcDataChannel>,
    /// HTTP request for signaling (created internally)
    http_request: Gd<HttpRequest>,
    /// Internal signal handler node
    signal_handler: Gd<SignalHandler>,
    /// Signaling server URL (e.g., "http://localhost:8080")
    signaling_url: String,
    /// Connection status
    connected: bool,
    /// Signaling state: waiting for offer, waiting for answer, etc.
    signaling_state: SignalingState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SignalingState {
    NotStarted,
    WaitingForOffer,
    WaitingForAnswer,
    Complete,
}

const DATA_CHANNEL_NAME: &str = "game";

/// Represents an SDP (Session Description Protocol) description
#[derive(Debug, Clone)]
struct SdpDescription {
    type_: GString,
    sdp: GString,
}

/// Minimal helper Node to handle WebRTC and HTTP signals internally
#[derive(GodotClass)]
#[class(base=Node)]
struct SignalHandler {
    pending_offer: Rc<RefCell<Option<SdpDescription>>>,
    pending_answer: Rc<RefCell<Option<(i64, PackedByteArray)>>>,
    #[base]
    base: Base<Node>,
}

#[godot_api]
impl INode for SignalHandler {
    fn init(base: Base<Node>) -> Self {
        Self {
            pending_offer: Rc::new(RefCell::new(None)),
            pending_answer: Rc::new(RefCell::new(None)),
            base,
        }
    }
}

#[godot_api]
impl SignalHandler {
    #[func]
    fn on_session_description_created(&mut self, type_: GString, sdp: GString) {
        *self.pending_offer.borrow_mut() = Some(SdpDescription { type_, sdp });
    }

    #[func]
    fn on_http_request_completed(
        &mut self,
        _result: i64,
        response_code: i64,
        _headers: PackedStringArray,
        body: PackedByteArray,
    ) {
        *self.pending_answer.borrow_mut() = Some((response_code, body));
    }
}

impl WebRTCClientTransport {
    /// Create a new WebRTC client transport
    /// Initializes peer connection with ICE servers, creates a data channel, and sets up HTTP signaling
    ///
    /// # Arguments
    /// * `signaling_url` - HTTP URL for signaling server (e.g., "http://localhost:8080")
    pub fn new(signaling_url: String) -> Result<Self, TransportError> {
        let mut peer_connection = WebRtcPeerConnection::new_gd();

        // Initialize with ICE servers (same as GDScript: pc.initialize({"iceServers": [...]}))
        let ice_servers = vdict! {
            "iceServers": vec![vdict! {
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

        // Create data channel (same as GDScript: channel = pc.create_data_channel("game"))
        let data_channel = peer_connection
            .create_data_channel(DATA_CHANNEL_NAME)
            .ok_or_else(|| TransportError::Other("Failed to create data channel".to_string()))?;

        // Create HTTP request for signaling
        let http_request = HttpRequest::new_alloc();

        // Create signal handler node
        let signal_handler = SignalHandler::new_alloc();

        let mut transport = Self {
            peer_connection: peer_connection.clone(),
            data_channel,
            http_request: http_request.clone(),
            signal_handler: signal_handler.clone(),
            signaling_url,
            connected: false,
            signaling_state: SignalingState::NotStarted,
        };

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

        // Connect HTTP request signal to handler
        let mut http_obj = http_request.clone().upcast::<Object>();
        let http_callable = handler_obj.callable("on_http_request_completed");

        match http_obj.connect("request_completed", &http_callable) {
            Error::OK => {}
            err => {
                return Err(TransportError::Other(format!(
                    "Failed to connect request_completed signal: {:?}",
                    err
                )));
            }
        }

        // Start signaling process
        transport.start_signaling()?;

        Ok(transport)
    }

    /// Start signaling process - creates offer and sends to server
    fn start_signaling(&mut self) -> Result<(), TransportError> {
        self.signaling_state = SignalingState::WaitingForOffer;

        // Clear any previous offer
        *self.signal_handler.bind().pending_offer.borrow_mut() = None;

        // Create offer (triggers session_description_created signal)
        match self.peer_connection.create_offer() {
            Error::OK => Ok(()),
            err => Err(TransportError::Other(format!(
                "Failed to create offer: {:?}",
                err
            ))),
        }
    }

    /// Send offer to signaling server via HTTP POST
    fn send_offer_to_server(&mut self, offer: SdpDescription) -> Result<(), TransportError> {
        // Create JSON body
        let json_bytes = Self::create_offer_json(&offer)?;

        // Prepare headers
        let mut headers = PackedStringArray::new();
        headers.push("Content-Type: application/json");

        // Send POST request with body using HttpRequest
        match self
            .http_request
            .request_raw_ex(&self.signaling_url)
            .custom_headers(&headers)
            .method(Method::POST)
            .request_data_raw(&json_bytes)
            .done()
        {
            Error::OK => Ok(()),
            err => Err(TransportError::Other(format!(
                "Failed to send HTTP request: {:?}",
                err
            ))),
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

        // Check if offer was created and send it via HTTP
        if self.signaling_state == SignalingState::WaitingForOffer {
            // Check if we have a pending offer from the signal
            let offer = self.signal_handler.bind().pending_offer.borrow_mut().take();
            if let Some(offer) = offer {
                // Set local description first
                match self
                    .peer_connection
                    .set_local_description(&offer.type_, &offer.sdp)
                {
                    Error::OK => {
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

        // Check HTTP request status
        if self.signaling_state == SignalingState::WaitingForAnswer {
            // Check if we have a pending answer from the signal
            let answer = self
                .signal_handler
                .bind()
                .pending_answer
                .borrow_mut()
                .take();
            if let Some((response_code, body)) = answer {
                if response_code == 200 {
                    self.handle_answer_response(body)?;
                } else {
                    return Err(TransportError::Other(format!(
                        "Signaling failed: {}",
                        response_code
                    )));
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
