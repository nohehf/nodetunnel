extends Control

const UDP_RELAY = "localhost:8080"
const WEB_RTC_SIGNALING = "http://0.0.0.0:8081/signaling"

@onready var clients_container: GridContainer = $VBoxContainer/Clients
@onready var add_client_button: Button = $VBoxContainer/AddClientButton
@onready var transport_option: OptionButton = $VBoxContainer/TransportOption
@onready var relay_address_input: LineEdit = $VBoxContainer/RelayAddressInput
@onready var webrtc_signaling_input: LineEdit = $VBoxContainer/WebRTCSignalingInput
@onready var app_id_input: LineEdit = $VBoxContainer/AppIdInput

var client_scene = preload("res://client.tscn")
var client_counter: int = 0

func _ready() -> void:
	# Setup initial values
	relay_address_input.text = UDP_RELAY
	webrtc_signaling_input.text = WEB_RTC_SIGNALING
	app_id_input.text = "app_id_1234"
	
	# Setup transport option
	transport_option.add_item("UDP")
	transport_option.add_item("WebRTC")
	transport_option.selected = 0

func _on_add_client_pressed() -> void:
	var client = client_scene.instantiate()
	client_counter += 1
	client.client_name = "Client %d" % client_counter
	client.transport_type = transport_option.selected
	client.relay_address = relay_address_input.text.strip_edges()
	client.webrtc_signaling = webrtc_signaling_input.text.strip_edges()
	client.app_id = app_id_input.text.strip_edges()
	
	# Connect client signals
	client.client_connected.connect(_on_client_connected)
	client.client_disconnected.connect(_on_client_disconnected)
	client.room_joined.connect(_on_room_joined)
	client.rooms_received.connect(_on_rooms_received)
	client.remove_requested.connect(_on_client_remove_requested)
	
	clients_container.add_child(client)
	print("Added client: %s" % client.client_name)

func _on_client_connected(client_id: String) -> void:
	print("Client connected: %s" % client_id)

func _on_client_disconnected(client_id: String) -> void:
	print("Client disconnected: %s" % client_id)

func _on_room_joined(room_id: String) -> void:
	print("Room joined: %s" % room_id)

func _on_rooms_received(rooms: Array) -> void:
	print("Rooms received: %d rooms" % rooms.size())

func _on_client_remove_requested(client_node: Node) -> void:
	var client_name_str = client_node.client_name
	
	# Disconnect all signals (check first to avoid errors)
	if client_node.client_connected.is_connected(_on_client_connected):
		client_node.client_connected.disconnect(_on_client_connected)
	if client_node.client_disconnected.is_connected(_on_client_disconnected):
		client_node.client_disconnected.disconnect(_on_client_disconnected)
	if client_node.room_joined.is_connected(_on_room_joined):
		client_node.room_joined.disconnect(_on_room_joined)
	if client_node.rooms_received.is_connected(_on_rooms_received):
		client_node.rooms_received.disconnect(_on_rooms_received)
	if client_node.remove_requested.is_connected(_on_client_remove_requested):
		client_node.remove_requested.disconnect(_on_client_remove_requested)
	
	# Remove from container and free
	clients_container.remove_child(client_node)
	client_node.queue_free()
	print("Removed client: %s" % client_name_str)
