extends Control
## Condensate multiplayer demo: host or join a lobby, invite friends, and chat over RPCs.
## Everything is built in code so the whole example is in this one file.

var _status: Label
var _lobbies: VBoxContainer
var _friends: VBoxContainer
var _log: RichTextLabel
var _input: LineEdit


func _ready() -> void:
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	for side in ["left", "right", "top", "bottom"]:
		margin.add_theme_constant_override("margin_" + side, 16)
	add_child(margin)
	var root := VBoxContainer.new()
	margin.add_child(root)

	_status = Label.new()
	root.add_child(_status)
	var buttons := HBoxContainer.new()
	root.add_child(buttons)
	_button(buttons, "Host a lobby", _host)
	_button(buttons, "Leave", Condensate.leave_lobby)
	_button(buttons, "Refresh", _refresh)

	var columns := HBoxContainer.new()
	columns.custom_minimum_size.y = 200
	root.add_child(columns)
	_lobbies = _column(columns, "Lobbies you can join")
	_friends = _column(columns, "Friends")

	_log = RichTextLabel.new()
	_log.bbcode_enabled = true
	_log.scroll_following = true
	_log.size_flags_vertical = SIZE_EXPAND_FILL
	root.add_child(_log)
	_input = LineEdit.new()
	_input.placeholder_text = "Say something to the lobby"
	_input.text_submitted.connect(_say)
	root.add_child(_input)

	Condensate.connected.connect(func(user): _status.text = "Online as %s" % user.display_name; _refresh())
	Condensate.connection_failed.connect(func(message): _status.text = message)
	Condensate.disconnected.connect(func(): _status.text = "Lost the connection to Condensate")
	Condensate.error.connect(func(message): _print("[color=#ff8c8c]%s[/color]" % message))
	Condensate.join_requested.connect(_join)
	Condensate.lobby_left.connect(func(reason): _print("Left the lobby (%s)" % reason); _refresh())
	Condensate.member_joined.connect(func(m): _print("%s joined" % m.display_name))
	multiplayer.peer_connected.connect(func(id): _print("Peer %d connected" % id))
	multiplayer.peer_disconnected.connect(func(id): _print("Peer %d disconnected" % id))
	_status.text = "Connecting to Condensate..."


func _host() -> void:
	if (await Condensate.create_lobby(8)).is_empty():
		return
	multiplayer.multiplayer_peer = Condensate.create_peer()
	_print("Hosting. Friends can join from Condensate, or invite them below.")
	_refresh()


func _join(lobby_id: String) -> void:
	if (await Condensate.join_lobby(lobby_id)).is_empty():
		return
	multiplayer.multiplayer_peer = Condensate.create_peer()
	_print("Joined %s's lobby" % Condensate.lobby.members[0].display_name)


func _refresh() -> void:
	if not Condensate.is_online():
		return
	for child in _lobbies.get_children() + _friends.get_children():
		if child is Button or child is HBoxContainer:
			child.queue_free()
	for lobby in await Condensate.list_lobbies():
		_button(_lobbies, "Join %s (%d/%d)" % [lobby.host.display_name, lobby.members, lobby.max_members], _join.bind(lobby.id))
	for friend in await Condensate.get_friends():
		var row := HBoxContainer.new()
		var label := Label.new()
		label.text = "%s - %s" % [friend.display_name, friend.status]
		row.add_child(label)
		if not Condensate.lobby.is_empty() and friend.status != "offline":
			_button(row, "Invite", _invite.bind(friend))
		_friends.add_child(row)


func _invite(friend: Dictionary) -> void:
	if await Condensate.invite(friend.username):
		_print("Invited %s" % friend.display_name)


func _say(text: String) -> void:
	_input.clear()
	if Condensate.lobby.is_empty():
		_print("Host or join a lobby first.")
	elif text.strip_edges() != "":
		_chat.rpc(Condensate.user.display_name, text)


@rpc("any_peer", "call_local", "reliable")
func _chat(sender: String, text: String) -> void:
	_print("[b]%s[/b]: %s" % [sender, text])


func _print(text: String) -> void:
	_log.append_text(text + "\n")


func _button(parent: Node, text: String, action: Callable) -> void:
	var b := Button.new()
	b.text = text
	b.pressed.connect(action)
	parent.add_child(b)


func _column(parent: Node, title: String) -> VBoxContainer:
	var col := VBoxContainer.new()
	col.size_flags_horizontal = SIZE_EXPAND_FILL
	var label := Label.new()
	label.text = title
	col.add_child(label)
	parent.add_child(col)
	return col
