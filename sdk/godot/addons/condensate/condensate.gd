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
## The lobby's feedback recording started or stopped. [param by] is who did it (display name).
signal feedback_changed(recording: bool, by: String)
## A feedback screenshot was taken and sent.
signal screenshot_taken

signal _reply(rid: int, message: Dictionary)

enum State { DISCONNECTED, CONNECTING, CONNECTED }

const GAME_ID_SETTING := "condensate/game_id"
const SCREENSHOT_KEY_SETTING := "condensate/feedback_screenshot_key"
const _HEADER := 6
const _MODE_SCREENSHOT := 4
const _MAX_BUFFERED := 4096
const _MAX_SHOT_WIDTH := 1920

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
## The feedback recording id while one includes this game (this lobby's, or the player's voice
## room's in the Condensate app), else empty.
var feedback_session := ""
## Optional: a Callable returning a Dictionary about the current moment (level, position, ...).
## It's saved with each feedback screenshot so the developer knows where it was taken.
## [codeblock]
## Condensate.feedback_context = func(): return {"level": level_name, "position": str(player.position)}
## [/codeblock]
var feedback_context := Callable()

var _ws := WebSocketPeer.new()
var _rid := 0
var _peer: CondensateMultiplayerPeer
var _inbox: Array = []
var _screenshot_key := KEY_F12
var _overlay: CanvasLayer
var _rec_label: Label
var _banner: Label
var _banner_timer: SceneTreeTimer
var _shooting := false
# Feedback recordings that include this game: its own lobby's, and the Condensate app's voice
# room's (when the room's host is recording this game). Either one turns recording on.
var _lobby_session := ""
var _room_session := ""


func _ready() -> void:
	process_mode = Node.PROCESS_MODE_ALWAYS
	var key_name := str(ProjectSettings.get_setting(SCREENSHOT_KEY_SETTING, "F12"))
	var key := OS.find_keycode_from_string(key_name)
	if key != KEY_NONE:
		_screenshot_key = key
	_build_overlay()
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


## Host only: start or stop recording this lobby for feedback. While it's on, Condensate saves
## everyone's voice chat (transcribed afterwards) and players can press the screenshot key
## (F12 by default) to mark a moment. The game's developer gets it all in Condensate Studio.
## Players can also start and stop it from the Condensate app.
func set_feedback(on: bool) -> bool:
	return (await _request({"op": "set_feedback", "on": on})).get("op") == "ok"


func is_recording_feedback() -> bool:
	return feedback_session != ""


## Save a feedback screenshot of the current frame (what the screenshot key does). Only works
## while feedback is being recorded (by this lobby, or by the player's voice room in the
## Condensate app). [param note] is saved with it.
func take_feedback_screenshot(note := "") -> void:
	if not is_recording_feedback() or _shooting:
		return
	_shooting = true
	_overlay.visible = false
	await RenderingServer.frame_post_draw
	var texture := get_viewport().get_texture()
	var image: Image = texture.get_image() if texture else null
	_overlay.visible = true
	_shooting = false
	if image == null or image.is_empty():
		error.emit("couldn't capture the screen")
		return
	if image.get_width() > _MAX_SHOT_WIDTH:
		var h := int(image.get_height() * float(_MAX_SHOT_WIDTH) / image.get_width())
		image.resize(_MAX_SHOT_WIDTH, h, Image.INTERPOLATE_BILINEAR)
	var meta := {
		"scene": get_tree().current_scene.scene_file_path if get_tree().current_scene else "",
		"version": str(ProjectSettings.get_setting("application/config/version", "")),
		"note": note,
	}
	if feedback_context.is_valid():
		var context = feedback_context.call()
		if context is Dictionary:
			meta["context"] = context
	var meta_bytes := JSON.stringify(meta).to_utf8_buffer()
	var frame := PackedByteArray()
	frame.resize(_HEADER + 4)
	frame[5] = _MODE_SCREENSHOT
	frame.encode_u32(_HEADER, meta_bytes.size())
	frame.append_array(meta_bytes)
	frame.append_array(image.save_jpg_to_buffer(0.85))
	_ws.send(frame, WebSocketPeer.WRITE_MODE_BINARY)
	_show_banner("Screenshot saved for feedback", 2.0)
	screenshot_taken.emit()


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


func _input(event: InputEvent) -> void:
	if is_recording_feedback() and event is InputEventKey and event.pressed and not event.echo \
			and event.keycode == _screenshot_key:
		get_viewport().set_input_as_handled()
		take_feedback_screenshot()


# A small "recording" indicator in the corner, and a banner for announcements. Players must be
# able to see when they're being recorded, so it's always shown while recording.
func _build_overlay() -> void:
	_overlay = CanvasLayer.new()
	_overlay.layer = 128
	add_child(_overlay)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0, 0, 0, 0.6)
	style.set_corner_radius_all(6)
	style.set_content_margin_all(8)
	_rec_label = Label.new()
	_rec_label.add_theme_stylebox_override("normal", style)
	_rec_label.add_theme_color_override("font_color", Color(1, 0.45, 0.45))
	_rec_label.add_theme_font_size_override("font_size", 14)
	_rec_label.set_anchors_and_offsets_preset(Control.PRESET_TOP_RIGHT, Control.PRESET_MODE_MINSIZE, 12)
	_rec_label.grow_horizontal = Control.GROW_DIRECTION_BEGIN
	_rec_label.visible = false
	_overlay.add_child(_rec_label)
	_banner = Label.new()
	_banner.add_theme_stylebox_override("normal", style)
	_banner.add_theme_font_size_override("font_size", 16)
	_banner.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_banner.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_banner.set_anchors_and_offsets_preset(Control.PRESET_CENTER_TOP, Control.PRESET_MODE_MINSIZE, 48)
	_banner.custom_minimum_size.x = 520
	_banner.grow_horizontal = Control.GROW_DIRECTION_BOTH
	_banner.visible = false
	_overlay.add_child(_banner)


func _update_feedback(by: String) -> void:
	_set_feedback_session(_lobby_session if _lobby_session != "" else _room_session, by)


func _set_feedback_session(session: String, by: String) -> void:
	var was := feedback_session
	feedback_session = session
	var key := OS.get_keycode_string(_screenshot_key)
	_rec_label.text = "● REC   %s: screenshot" % key
	_rec_label.visible = session != ""
	if session != "" and was == "":
		var who := "%s started recording" % by if by != "" else "Recording"
		_show_banner("%s for feedback. Your voice chat and %s screenshots go to the game's developer. You can opt out in the Condensate app." % [who, key], 8.0)
	elif session == "" and was != "":
		_show_banner("Feedback recording stopped", 3.0)
	if session != was:
		feedback_changed.emit(session != "", by)


func _show_banner(text: String, seconds: float) -> void:
	_banner.text = text
	_banner.visible = true
	_banner_timer = get_tree().create_timer(seconds, true, false, true)
	var timer := _banner_timer
	await timer.timeout
	if _banner_timer == timer:
		_banner.visible = false


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
			var session = lobby.get("feedback")
			_lobby_session = "" if session == null else str(session)
			_update_feedback("")
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
		"member_updated":
			if not lobby.is_empty():
				var updated: Dictionary = m["member"]
				for i in lobby["members"].size():
					if int(lobby["members"][i]["id"]) == int(updated["id"]):
						lobby["members"][i] = updated
		"feedback":
			if not lobby.is_empty():
				var session = m.get("session")
				lobby["feedback"] = session
				_lobby_session = "" if session == null else str(session)
				_update_feedback(str(m["by"]["display_name"]))
		"room_feedback":
			# From the app: the voice room is recording this game (or stopped).
			var session = m.get("session")
			_room_session = "" if session == null else str(session)
			_update_feedback(str(m.get("by", "")))
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
	_lobby_session = ""
	_update_feedback("")
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
