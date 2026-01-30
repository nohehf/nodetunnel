extends Node2D

const UDP = "localhost:8080"
const WEB_RTC_SIGNALING = "http://0.0.0.0:8081/signaling"

#func _ready() -> void:
	#print("Starting")
	#var peer := NodeTunnelPeer.new()
	#print("connect_to_relay")
	#peer.connect_to_relay(UDP, "eat_the_rich_1234")
	#multiplayer.multiplayer_peer = peer
	#
	#print("Authenticating")
	#await peer.authenticated
	#print("Authenticated!")

func _ready() -> void:
	print("Starting")
	var peer := WebNodeTunnelPeer.new()
	print("connect_to_relay")
	peer.connect_to_relay(WEB_RTC_SIGNALING, "eat_the_rich_1234")
	multiplayer.multiplayer_peer = peer
	
	print("Authenticating")
	await peer.authenticated
	print("Authenticated!")
