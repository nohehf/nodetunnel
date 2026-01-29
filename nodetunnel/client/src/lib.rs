mod node_tunnel_peer;
mod relay_client;
mod transport;
mod web_node_tunnel_peer;

use godot::prelude::*;

struct NodeTunnel;

#[gdextension]
unsafe impl ExtensionLibrary for NodeTunnel {}
