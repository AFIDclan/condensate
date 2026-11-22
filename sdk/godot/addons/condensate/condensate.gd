extends Node
## Condensate multiplayer: who's playing, lobbies and invites, plus a MultiplayerPeer that sends
## your game's packets through Condensate. The plugin adds this as the `Condensate` autoload.
##
## [codeblock]
## # Host
## var lobby := await Condensate.create_lobby(4)
## multiplayer.multiplayer_peer = Condensate.create_peer()
##
## # Join (from a lobby list, or when the player accepts an invite in Condensate)
## Condensate.join_requested.connect(func(id): join(id))
## func join(id):
##     if not (await Condensate.join_lobby(id)).is_empty():
##         multiplayer.multiplayer_peer = Condensate.create_peer()
## [/codeblock]
##
## Your existing RPCs, MultiplayerSpawner and MultiplayerSynchronizer work unchanged: the host
## is peer 1, as with ENet.

## Connected to Condensate. [param user] has [code]username[/code] and [code]display_name[/code].
signal connected(user: Dictionary)
## Couldn't connect: Condensate isn't running, isn't logged in, or the game id is wrong.
signal connection_failed(message: String)
## Lost the connection to Condensate after being connected.
signal disconnected
## You created or joined a lobby.
signal lobby_joined(lobby: Dictionary)
## You're no longer in a lobby. [param reason] is [code]left[/code], [code]host_left[/code],
## [code]kicked[/code] or [code]disconnected[/code].
signal lobby_left(reason: String)
## Someone joined your lobby. [param member] has [code]id[/code], [code]username[/code] and
## [code]display_name[/code].
signal member_joined(member: Dictionary)
signal member_left(member_id: int)
signal lobby_data_changed(data: Dictionary)
## The player accepted an invite or clicked Join on a friend in Condensate. Join this lobby.
## When Condensate launched the game for this, it arrives right after [signal connected].
signal join_requested(lobby_id: String)
## A request failed, or Condensate reported a problem.
signal error(message: String)

signal _reply(rid: int, message: Dictionary)

enum State { DISCONNECTED, CONNECTING, CONNECTED }

const GAME_ID_SETTING := "condensate/game_id"
const _HEADER := 6
const _MAX_BUFFERED := 4096

var state := State.DISCONNECTED
## You: [code]username[/code], [code]display_name[/code].
var user: Dictionary = {}
## This game: [code]slug[/code], [code]title[/code].
var game: Dictionary = {}
## The lobby you're in, or empty: [code]id[/code], [code]members[/code], [code]max_members[/code],
## [code]visibility[/code], [code]data[/code].
var lobby: Dictionary = {}
## Your member id in the lobby (1 = host), or 0.
var member_id := 0
## True when Condensate started the game (rather than the editor).
var launched_by_condensate := false
## The last connection problem, for showing to the player.
var last_error := ""

var _ws := WebSocketPeer.new()
var _rid := 0
var _peer: CondensateMultiplayerPeer
var _inbox: Array = []


func _ready() -> void:
	connect_to_condensate()


## Called automatically at startup. Call again to retry after [signal connection_failed].
func connect_to_condensate() -> void:
	if state != State.DISCONNECTED:
		return
	var api := OS.get_environment("CONDENSATE_API")
	var token := OS.get_environment("CONDENSATE_TOKEN")
	var game_id := OS.get_environment("CONDENSATE_GAME")
	launched_by_condensate = api != ""
	if game_id == "":
		game_id = str(ProjectSettings.get_setting(GAME_ID_SETTING, ""))
	if api == "":
		# Run from the editor: find the Condensate app running on this PC.
		var info := _read_app_file()
		api = str(info.get("api", ""))
		token = str(info.get("token", ""))
	if api == "":
		_fail.call_deferred("Condensate isn't running. Start it and log in.")
		return
	if game_id == "":
		_fail.call_deferred("Set your game's Condensate id in Project Settings > Condensate > Game Id.")
		return
	_ws = WebSocketPeer.new()
	_ws.inbound_buffer_size = 4 << 20
	_ws.outbound_buffer_size = 4 << 20
	_ws.max_queued_packets = 8192
	var sep := "&" if "?" in api else "?"
	var err := _ws.connect_to_url("%s%stoken=%s&game=%s" % [api, sep, token.uri_encode(), game_id.uri_encode()])
	if err != OK:
		_fail.call_deferred("Couldn't connect to Condensate (error %d)." % err)
		return
	state = State.CONNECTING
	last_error = ""


func is_online() -> bool:
	return state == State.CONNECTED


func is_host() -> bool:
	return member_id == 1


# ---- lobbies --------------------------------------------------------------------------------

## Create a lobby and become its host (member 1). Your friends see a Join button in Condensate
## unless [param invite_only]. Returns the lobby, or an empty Dictionary on failure.
func create_lobby(max_members := 8, invite_only := false, data := {}) -> Dictionary:
	var r := await _request({
		"op": "create_lobby",
		"max_members": max_members,
		"visibility": "invite_only" if invite_only else "friends",
		"data": data,
	})
	return lobby if r.get("op") == "lobby" else {}


## Join a lobby by id. Returns the lobby, or an empty Dictionary on failure.
func join_lobby(lobby_id: String) -> Dictionary:
	var r := await _request({"op": "join_lobby", "lobby": lobby_id})
	return lobby if r.get("op") == "lobby" else {}


## Leave the current lobby. When the host leaves, the lobby closes for everyone.
func leave_lobby() -> void:
	if lobby.is_empty():
		return
	_leave_local("left")
	if is_online():
		_ws.send_text(JSON.stringify({"op": "leave_lobby"}))


## Invite a friend (by username) to your lobby; they get a notification in Condensate.
func invite(username: String) -> bool:
	return (await _request({"op": "invite", "user": username})).get("op") == "ok"


## Host only: replace the lobby's data (a small Dictionary everyone in the lobby can read).
func set_lobby_data(data: Dictionary) -> bool:
	return (await _request({"op": "set_lobby_data", "data": data})).get("op") == "ok"


## Host only: remove someone from the lobby.
func kick(id: int) -> bool:
	return (await _request({"op": "kick", "member": id})).get("op") == "ok"


## Lobbies for this game you can join (your friends' and ones you're invited to). Each has
## [code]id[/code], [code]host[/code] ({username, display_name}), [code]members[/code],
## [code]max_members[/code] and [code]data[/code].
func list_lobbies() -> Array:
	return (await _request({"op": "list_lobbies"})).get("lobbies", [])


## Your friends: [code]username[/code], [code]display_name[/code], [code]status[/code]
## (online, in_game or offline), [code]game[/code] and [code]lobby[/code] (a lobby of theirs in
## this game you can join, or null).
func get_friends() -> Array:
	return (await _request({"op": "list_friends"})).get("friends", [])


## A MultiplayerPeer for the current lobby: assign it to [code]multiplayer.multiplayer_peer[/code].
## Make a new one for each lobby.
func create_peer() -> CondensateMultiplayerPeer:
	if lobby.is_empty():
		push_error("Condensate.create_peer(): create or join a lobby first")
		return null
	_peer = CondensateMultiplayerPeer.new()
	var ids: Array[int] = []
	for m in lobby.get("members", []):
		ids.append(int(m["id"]))
	_peer._setup(self, member_id, ids)
	for p in _inbox:
		_peer._receive(p[0], p[1], p[2], p[3])
	_inbox.clear()
	return _peer


# ---- internals ------------------------------------------------------------------------------

func _process(_delta: float) -> void:
	if state == State.DISCONNECTED:
		return
	_ws.poll()
	while _ws.get_available_packet_count() > 0:
		var packet := _ws.get_packet()
		if _ws.was_string_packet():
			_on_text(packet.get_string_from_utf8())
		else:
			_on_packet(packet)
	if _ws.get_ready_state() == WebSocketPeer.STATE_CLOSED:
		var was := state
		state = State.DISCONNECTED
		_leave_local("disconnected")
		_reply.emit(0, {"op": "error", "message": "not connected to Condensate"})
		if was == State.CONNECTING:
			_fail(last_error if last_error != "" else "Couldn't connect to Condensate. Is it running?")
		else:
			disconnected.emit()


func _fail(message: String) -> void:
	last_error = message
	push_warning("Condensate: " + message)
	connection_failed.emit(message)


func _request(command: Dictionary) -> Dictionary:
	if not is_online():
		var message := "not connected to Condensate"
		error.emit(message)
		return {"op": "error", "message": message}
	_rid += 1
	var rid := _rid
	command["rid"] = rid
	_ws.send_text(JSON.stringify(command))
	while true:
		var r: Array = await _reply
		if r[0] == rid or r[0] == 0:
			if r[1].get("op") == "error":
				error.emit(str(r[1].get("message", "")))
			return r[1]
	return {}


func _on_text(text: String) -> void:
	var m = JSON.parse_string(text)
	if not m is Dictionary:
		return
	match str(m.get("op", "")):
		"welcome":
			user = m["you"]
			game = m["game"]
			state = State.CONNECTED
			connected.emit(user)
		"lobby":
			_leave_local("left")
			lobby = m["lobby"]
			member_id = int(m["you"])
			lobby_joined.emit(lobby)
		"member_joined":
			if not lobby.is_empty():
				lobby["members"].append(m["member"])
				if _peer:
					_peer._member_joined(int(m["member"]["id"]))
				member_joined.emit(m["member"])
		"member_left":
			var id := int(m["member"])
			if not lobby.is_empty():
				lobby["members"] = lobby["members"].filter(func(x): return int(x["id"]) != id)
				if _peer:
					_peer._member_left(id)
				member_left.emit(id)
		"lobby_data":
			if not lobby.is_empty():
				lobby["data"] = m["data"]
				lobby_data_changed.emit(m["data"])
		"lobby_closed":
			_leave_local(str(m.get("reason", "left")))
		"join_requested":
			join_requested.emit(str(m["lobby"]))
		"error":
			if m.get("rid") == null:
				last_error = str(m.get("message", ""))
				if state == State.CONNECTED:
					error.emit(last_error)
	if m.get("rid") != null:
		_reply.emit(int(m["rid"]), m)


func _on_packet(frame: PackedByteArray) -> void:
	if lobby.is_empty() or frame.size() < _HEADER:
		return
	var from := frame.decode_u32(0)
	var payload := frame.slice(_HEADER)
	if _peer:
		_peer._receive(from, frame[4], frame[5], payload)
	elif _inbox.size() < _MAX_BUFFERED:
		_inbox.append([from, frame[4], frame[5], payload])


func _send_packet(to: int, channel: int, mode: int, payload: PackedByteArray) -> Error:
	if not is_online() or lobby.is_empty():
		return ERR_UNCONFIGURED
	var frame := PackedByteArray()
	frame.resize(_HEADER)
	frame.encode_u32(0, to)
	frame[4] = channel
	frame[5] = mode
	frame.append_array(payload)
	return _ws.send(frame, WebSocketPeer.WRITE_MODE_BINARY)


func _leave_local(reason: String) -> void:
	if lobby.is_empty():
		return
	lobby = {}
	member_id = 0
	_inbox.clear()
	if _peer:
		_peer._lobby_ended()
		_peer = null
	lobby_left.emit(reason)


func _read_app_file() -> Dictionary:
	var base := ""
	match OS.get_name():
		"Windows":
			base = OS.get_environment("APPDATA")
		"macOS":
			base = OS.get_environment("HOME").path_join("Library/Application Support")
		_:
			base = OS.get_environment("XDG_CONFIG_HOME")
			if base == "":
				base = OS.get_environment("HOME").path_join(".config")
	var path := base.path_join("com.condensate.app").path_join("game-api.json")
	if not FileAccess.file_exists(path):
		return {}
	var data = JSON.parse_string(FileAccess.get_file_as_string(path))
	return data if data is Dictionary else {}
