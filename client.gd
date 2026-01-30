extends Control

signal client_connected(client_id: String)
signal client_disconnected(client_id: String)
signal room_joined(room_id: String)
signal rooms_received(rooms: Array)
signal remove_requested(client_node: Node)

enum TransportType {
	UDP,
	WEBRTC
}

@export var transport_type: TransportType = TransportType.UDP
@export var relay_address: String = "localhost:8080"
@export var webrtc_signaling: String = "http://0.0.0.0:8081/signaling"
@export var app_id: String = "app_id_1234"
@export var client_name: String = "Client"

var peer: MultiplayerPeer
var multiplayer_api: MultiplayerAPI
var connected_to_relay: bool = false
var current_room_id: String = ""

@onready var name_label: Label = $VBoxContainer/NameContainer/NameLabel
@onready var transport_label: Label = $VBoxContainer/TransportLabel
@onready var status_label: Label = $VBoxContainer/StatusLabel
@onready var room_label: Label = $VBoxContainer/RoomContainer/RoomLabel
@onready var connect_button: Button = $VBoxContainer/ConnectButton
@onready var disconnect_button: Button = $VBoxContainer/DisconnectButton
@onready var host_button: Button = $VBoxContainer/HostButton
@onready var join_button: Button = $VBoxContainer/JoinButton
@onready var leave_button: Button = $VBoxContainer/LeaveButton
@onready var refresh_rooms_button: Button = $VBoxContainer/RefreshRoomsButton
@onready var room_id_input: LineEdit = $VBoxContainer/RoomIdInput
@onready var room_list: ItemList = $VBoxContainer/RoomList
@onready var copy_room_button: Button = $VBoxContainer/RoomContainer/CopyRoomButton
@onready var chat_log: TextEdit = $VBoxContainer/ChatLog
@onready var chat_input: LineEdit = $VBoxContainer/ChatInputContainer/ChatInput
@onready var send_button: Button = $VBoxContainer/ChatInputContainer/SendButton
@onready var remove_button: Button = $VBoxContainer/NameContainer/RemoveButton

func _ready() -> void:
	# Create separate MultiplayerAPI for this client
	multiplayer_api = MultiplayerAPI.create_default_interface()
	get_tree().set_multiplayer(multiplayer_api, get_path())
	
	# Setup UI
	name_label.text = client_name
	transport_label.text = "Transport: " + TransportType.keys()[transport_type]
	status_label.text = "Status: Disconnected"
	room_label.text = "Room: None"
	
	# Setup initial button states
	update_button_states()
	
	# Setup chat
	chat_log.editable = false
	chat_input.editable = false # Will be enabled when in a room
	# Note: Signal connections for buttons are handled in the scene file (client.tscn)
	
	# Connect multiplayer signals (will be set up after peer is connected)

func _on_connect_pressed() -> void:
	print("[%s] Connect button pressed" % client_name)
	if connected_to_relay:
		print("[%s] Already connected, ignoring connect request" % client_name)
		return
	
	status_label.text = "Status: Connecting..."
	
	match transport_type:
		TransportType.UDP:
			print("[%s] Creating UDP peer (NodeTunnelPeer)" % client_name)
			peer = NodeTunnelPeer.new()
			print("[%s] Connecting to relay: %s with app_id: %s" % [client_name, relay_address, app_id])
			var error = peer.connect_to_relay(relay_address, app_id)
			print("[%s] connect_to_relay() returned: %s" % [client_name, error])
			if error != OK:
				status_label.text = "Status: Connection Failed"
				print("[%s] ERROR: Connection failed with error code %s" % [client_name, error])
				return
		TransportType.WEBRTC:
			print("[%s] Creating WebRTC peer (WebNodeTunnelPeer)" % client_name)
			peer = WebNodeTunnelPeer.new()
			print("[%s] Connecting to relay: %s with app_id: %s" % [client_name, webrtc_signaling, app_id])
			var error = peer.connect_to_relay(webrtc_signaling, app_id)
			print("[%s] connect_to_relay() returned: %s" % [client_name, error])
			if error != OK:
				status_label.text = "Status: Connection Failed"
				print("[%s] ERROR: Connection failed with error code %s" % [client_name, error])
				return
	
	# Connect peer signals
	peer.authenticated.connect(_on_authenticated)
	peer.error.connect(_on_error)
	peer.room_connected.connect(_on_room_connected)
	peer.forced_disconnect.connect(_on_forced_disconnect)
	peer.rooms_received.connect(_on_rooms_received)
	peer.peer_connected.connect(_on_peer_connected)
	peer.peer_disconnected.connect(_on_peer_disconnected)
	
	multiplayer_api.multiplayer_peer = peer
	status_label.text = "Status: Authenticating..."
	
	# Connect multiplayer signals (disconnect first to avoid duplicates)
	if multiplayer_api.peer_connected.is_connected(_on_mp_peer_connected):
		multiplayer_api.peer_connected.disconnect(_on_mp_peer_connected)
	if multiplayer_api.peer_disconnected.is_connected(_on_mp_peer_disconnected):
		multiplayer_api.peer_disconnected.disconnect(_on_mp_peer_disconnected)
	
	multiplayer_api.peer_connected.connect(_on_mp_peer_connected)
	multiplayer_api.peer_disconnected.connect(_on_mp_peer_disconnected)

func _on_disconnect_pressed() -> void:
	if peer:
		# Disconnect peer signals
		if peer.authenticated.is_connected(_on_authenticated):
			peer.authenticated.disconnect(_on_authenticated)
		if peer.error.is_connected(_on_error):
			peer.error.disconnect(_on_error)
		if peer.room_connected.is_connected(_on_room_connected):
			peer.room_connected.disconnect(_on_room_connected)
		if peer.forced_disconnect.is_connected(_on_forced_disconnect):
			peer.forced_disconnect.disconnect(_on_forced_disconnect)
		if peer.rooms_received.is_connected(_on_rooms_received):
			peer.rooms_received.disconnect(_on_rooms_received)
		if peer.peer_connected.is_connected(_on_peer_connected):
			peer.peer_connected.disconnect(_on_peer_connected)
		if peer.peer_disconnected.is_connected(_on_peer_disconnected):
			peer.peer_disconnected.disconnect(_on_peer_disconnected)
		
		peer.close()
		multiplayer_api.multiplayer_peer = null
		peer = null
	
	# Disconnect multiplayer API signals
	if multiplayer_api.peer_connected.is_connected(_on_mp_peer_connected):
		multiplayer_api.peer_connected.disconnect(_on_mp_peer_connected)
	if multiplayer_api.peer_disconnected.is_connected(_on_mp_peer_disconnected):
		multiplayer_api.peer_disconnected.disconnect(_on_mp_peer_disconnected)
	
	connected_to_relay = false
	current_room_id = ""
	status_label.text = "Status: Disconnected"
	room_label.text = "Room: None"
	room_list.clear()
	chat_log.text = ""
	update_button_states()
	client_disconnected.emit(client_name)

func _on_host_pressed() -> void:
	if not peer or not connected_to_relay:
		return
	
	var metadata = "Hosted by " + client_name
	var error = peer.host_room(true, metadata)
	if error != OK:
		status_label.text = "Status: Failed to create room"
		return
	
	status_label.text = "Status: Creating room..."

func _on_join_pressed() -> void:
	print("[%s] Join button pressed" % client_name)
	if not peer:
		print("[%s] ERROR: peer is null" % client_name)
		return
	if not connected_to_relay:
		print("[%s] ERROR: not connected to relay (connected_to_relay=%s)" % [client_name, connected_to_relay])
		return
	
	var room_id = room_id_input.text.strip_edges()
	if room_id.is_empty():
		status_label.text = "Status: Please enter a room ID"
		print("[%s] ERROR: room_id is empty" % client_name)
		return
	
	var metadata = "Joining as " + client_name
	print("[%s] Calling peer.join_room(room_id='%s', metadata='%s')" % [client_name, room_id, metadata])
	print("[%s] Peer type: %s" % [client_name, peer.get_class()])
	print("[%s] Connection status: %s" % [client_name, peer.get_connection_status()])
	var error = peer.join_room(room_id, metadata)
	print("[%s] peer.join_room() returned: %s" % [client_name, error])
	if error != OK:
		status_label.text = "Status: Failed to join room"
		print("[%s] ERROR: join_room failed with error code %s" % [client_name, error])
		return
	
	status_label.text = "Status: Joining room..."
	print("[%s] Join room request sent successfully" % client_name)

func _on_leave_pressed() -> void:
	if not peer or not connected_to_relay:
		return
	
	_on_disconnect_pressed()
	_on_connect_pressed()

func _on_refresh_rooms_pressed() -> void:
	if not peer or not connected_to_relay:
		return
	
	var error = peer.get_rooms()
	if error != OK:
		status_label.text = "Status: Failed to refresh rooms"
		return
	
	status_label.text = "Status: Refreshing rooms..."

func _on_room_selected(index: int) -> void:
	if index < 0:
		return
	
	var selected_text = room_list.get_item_text(index)
	# Extract room ID from the display text (format: "Room ID: xyz")
	var parts = selected_text.split(": ")
	if parts.size() >= 2:
		room_id_input.text = parts[1]

func _on_authenticated() -> void:
	print("[%s] Authenticated signal received" % client_name)
	connected_to_relay = true
	status_label.text = "Status: Connected"
	update_button_states()
	client_connected.emit(client_name)
	print("[%s] Now connected to relay, ready for room operations" % client_name)

func _on_error(error_message: String) -> void:
	status_label.text = "Status: Error - " + error_message
	print("[%s] Error: %s" % [client_name, error_message])

func _on_room_connected() -> void:
	print("[%s] Room connected signal received" % client_name)
	if peer:
		current_room_id = peer.room_id
		print("[%s] Joined room: %s" % [client_name, current_room_id])
		room_label.text = "Room: " + current_room_id
		status_label.text = "Status: In Room"
		update_button_states()
		add_chat_message("System", "Joined room: " + current_room_id)
		room_joined.emit(current_room_id)
	else:
		print("[%s] ERROR: peer is null in _on_room_connected()" % client_name)

func _on_forced_disconnect() -> void:
	status_label.text = "Status: Forced Disconnect"
	_on_disconnect_pressed()

func _on_rooms_received(rooms: Array) -> void:
	room_list.clear()
	for room in rooms:
		var room_dict = room as Dictionary
		var room_id = room_dict.get("id", "")
		var metadata = room_dict.get("metadata", "")
		var display_text = "Room ID: %s | %s" % [room_id, metadata]
		room_list.add_item(display_text)
	
	status_label.text = "Status: Connected (%d rooms)" % rooms.size()
	rooms_received.emit(rooms)

func _on_peer_connected(peer_id: int) -> void:
	print("[%s] Peer connected: %d" % [client_name, peer_id])

func _on_peer_disconnected(peer_id: int) -> void:
	print("[%s] Peer disconnected: %d" % [client_name, peer_id])
	add_chat_message("System", "Peer %d disconnected" % peer_id)

func _on_mp_peer_connected(id: int) -> void:
	add_chat_message("System", "Peer %d connected" % id)

func _on_mp_peer_disconnected(id: int) -> void:
	add_chat_message("System", "Peer %d disconnected" % id)

func _on_send_pressed() -> void:
	send_chat_message()

func _on_chat_text_submitted(_text: String) -> void:
	send_chat_message()

func send_chat_message() -> void:
	if current_room_id.is_empty() or not connected_to_relay:
		return
	
	var message = chat_input.text.strip_edges()
	if message.is_empty():
		return
	
	chat_input.text = ""
	# Send to all peers including ourselves
	send_chat_rpc.rpc(message)

@rpc("any_peer", "call_local", "reliable")
func send_chat_rpc(message: String) -> void:
	# Use multiplayer singleton which refers to our custom MultiplayerAPI
	var sender_id = multiplayer.get_remote_sender_id()
	var sender_name = "Peer %d" % sender_id
	
	# If it's from ourselves (sender_id is 0 for local calls), use our client name
	if sender_id == 0 or sender_id == multiplayer.get_unique_id():
		sender_name = client_name
	
	add_chat_message(sender_name, message)

func add_chat_message(sender: String, message: String) -> void:
	var timestamp = Time.get_time_string_from_system()
	var formatted_message = "[%s] %s: %s\n" % [timestamp, sender, message]
	chat_log.text += formatted_message
	# Auto-scroll to bottom
	var line_count = chat_log.get_line_count()
	if line_count > 0:
		chat_log.set_caret_line(line_count - 1)
		chat_log.set_caret_column(chat_log.get_line(line_count - 1).length())
		# Force scroll to bottom by setting scroll_vertical
		chat_log.scroll_vertical = line_count - 1

func _on_copy_room_pressed() -> void:
	if current_room_id.is_empty():
		return
	
	DisplayServer.clipboard_set(current_room_id)
	add_chat_message("System", "Room ID copied to clipboard: " + current_room_id)

func _on_remove_pressed() -> void:
	# Disconnect first if connected
	if connected_to_relay:
		_on_disconnect_pressed()
	
	# Emit signal to parent to handle removal
	remove_requested.emit(self)

func update_button_states() -> void:
	connect_button.disabled = connected_to_relay
	disconnect_button.disabled = not connected_to_relay
	host_button.disabled = not connected_to_relay or not current_room_id.is_empty()
	join_button.disabled = not connected_to_relay or not current_room_id.is_empty()
	leave_button.disabled = not connected_to_relay or current_room_id.is_empty()
	refresh_rooms_button.disabled = not connected_to_relay
	copy_room_button.disabled = current_room_id.is_empty()
	send_button.disabled = current_room_id.is_empty()
	# Enable chat input only when in a room
	chat_input.editable = not current_room_id.is_empty() and connected_to_relay
	if chat_input.editable:
		chat_input.placeholder_text = "Type a message..."
	else:
		chat_input.placeholder_text = "Join a room to chat..."
