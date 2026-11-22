class_name CondensateMultiplayerPeer
extends MultiplayerPeerExtension
## A MultiplayerPeer whose packets go through Condensate. Get one from [code]Condensate.create_peer()[/code]
## after creating or joining a lobby, and assign it to [code]multiplayer.multiplayer_peer[/code].
##
## It behaves like ENet's client/server mode: the lobby host is the server (peer 1) and sees every
## other member; other members see only the host, and Godot relays anything between them through
## the host. When the lobby ends, the peer disconnects ([signal MultiplayerAPI.server_disconnected]
## fires on clients).

var _api: Node
var _id := 0
var _status := CONNECTION_CONNECTED
var _target := 0
var _channel := 0
var _mode := TRANSFER_MODE_RELIABLE
var _refusing := false
## Received packets: [from, channel, mode, payload]. The front one is the "current" packet.
var _queue: Array = []
## Packets from members not announced yet (they joined this frame); moved to _queue on poll.
var _held: Array = []
## Peers announced with peer_connected.
var _peers := {}
## Connect/disconnect signals to emit on the next poll (after the MultiplayerAPI is listening).
var _events: Array = []


func _setup(api: Node, my_id: int, member_ids: Array[int]) -> void:
	_api = api
	_id = my_id
	for id in member_ids:
		_member_joined(id)


# Godot's client/server model: the host sees everyone, everyone else sees only the host.
func _visible(id: int) -> bool:
	return id != _id and (_id == 1 or id == 1)


func _member_joined(id: int) -> void:
	if _visible(id) and not _refusing:
		_events.append([true, id])


func _member_left(id: int) -> void:
	_events.append([false, id])


func _lobby_ended() -> void:
	for id in _peers.keys():
		_events.append([false, id])
	_status = CONNECTION_DISCONNECTED


func _receive(from: int, channel: int, mode: int, payload: PackedByteArray) -> void:
	if _status != CONNECTION_CONNECTED:
		return
	if _peers.has(from):
		_queue.append([from, channel, mode, payload])
	elif _visible(from):
		_held.append([from, channel, mode, payload])


func _poll() -> void:
	var events := _events
	_events = []
	for e in events:
		var id: int = e[1]
		if e[0] and not _peers.has(id):
			_peers[id] = true
			peer_connected.emit(id)
		elif not e[0] and _peers.has(id):
			_peers.erase(id)
			_queue = _queue.filter(func(p): return p[0] != id)
			_held = _held.filter(func(p): return p[0] != id)
			peer_disconnected.emit(id)
	if not _held.is_empty():
		var waiting := []
		for p in _held:
			if _peers.has(p[0]):
				_queue.append(p)
			else:
				waiting.append(p)
		_held = waiting


func _get_available_packet_count() -> int:
	return _queue.size()


func _get_packet_script() -> PackedByteArray:
	if _queue.is_empty():
		return PackedByteArray()
	return _queue.pop_front()[3]


func _get_packet_peer() -> int:
	if _queue.is_empty():
		return 0
	return _queue[0][0]


func _get_packet_channel() -> int:
	if _queue.is_empty():
		return 0
	return _queue[0][1]


func _get_packet_mode() -> MultiplayerPeer.TransferMode:
	if _queue.is_empty():
		return TRANSFER_MODE_RELIABLE
	return _queue[0][2]


func _put_packet_script(buffer: PackedByteArray) -> Error:
	if _status != CONNECTION_CONNECTED:
		return ERR_UNCONFIGURED
	var t := _target
	if t > 0:
		return _api._send_packet(t, _channel, _mode, buffer)
	if _id != 1:
		# A client only talks to the host; Godot relays the rest.
		return OK if t == -1 else _api._send_packet(1, _channel, _mode, buffer)
	if t == 0:
		return _api._send_packet(0, _channel, _mode, buffer)
	# Everyone except -t.
	for id in _peers.keys():
		if id != -t:
			_api._send_packet(id, _channel, _mode, buffer)
	return OK


func _get_max_packet_size() -> int:
	return 1 << 20


func _set_target_peer(peer: int) -> void:
	_target = peer


func _set_transfer_channel(channel: int) -> void:
	_channel = channel


func _get_transfer_channel() -> int:
	return _channel


func _set_transfer_mode(mode: MultiplayerPeer.TransferMode) -> void:
	_mode = mode


func _get_transfer_mode() -> MultiplayerPeer.TransferMode:
	return _mode


func _get_unique_id() -> int:
	return _id


func _is_server() -> bool:
	return _id == 1


func _is_server_relay_supported() -> bool:
	return true


func _get_connection_status() -> MultiplayerPeer.ConnectionStatus:
	return _status


func _set_refuse_new_connections(enable: bool) -> void:
	_refusing = enable


func _is_refusing_new_connections() -> bool:
	return _refusing


func _disconnect_peer(peer: int, _force: bool) -> void:
	if _id == 1:
		_api.kick(peer)
	elif peer == 1:
		_api.leave_lobby()


func _close() -> void:
	if _status == CONNECTION_CONNECTED and _api and _api._peer == self:
		_api.leave_lobby()
	_status = CONNECTION_DISCONNECTED
