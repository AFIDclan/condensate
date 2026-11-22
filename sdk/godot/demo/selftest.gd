extends Node
## Automated check of the addon against a real server, used by scripts/godot-test.sh. Run three
## copies: `godot --headless res://demo/selftest.tscn -- host`, then `-- a` and `-- b` (each with
## CONDENSATE_API / CONDENSATE_TOKEN for a different user). Prints PASS or FAIL.

var role := ""
var hellos: Array = []
var done: Array = []


func _ready() -> void:
	role = OS.get_cmdline_user_args()[0]
	get_tree().create_timer(30).timeout.connect(func(): _fail("timed out"))
	Condensate.connection_failed.connect(func(m): _fail("connection failed: " + m))
	await Condensate.connected
	_log("connected as " + Condensate.user.username)
	if role == "host":
		await _host()
	else:
		await _client()


func _host() -> void:
	var lobby := await Condensate.create_lobby(4, false, {"map": "lobby"})
	if lobby.is_empty():
		_fail("create_lobby failed")
		return
	multiplayer.multiplayer_peer = Condensate.create_peer()
	var peers := []
	while peers.size() < 2:
		peers.append(await multiplayer.peer_connected)
	peers.sort()
	_check(peers == [2, 3], "host sees peers 2 and 3: %s" % [peers])
	await get_tree().create_timer(0.5).timeout
	await Condensate.set_lobby_data({"map": "arena"})
	while done.size() < 2:
		await get_tree().process_frame
	_check(hellos.size() == 2, "host got hello from both clients: %s" % [hellos])
	Condensate.leave_lobby()
	_pass()


func _client() -> void:
	var lobbies := []
	while lobbies.is_empty():
		lobbies = await Condensate.list_lobbies()
		if lobbies.is_empty():
			await get_tree().create_timer(0.2).timeout
	if (await Condensate.join_lobby(lobbies[0].id)).is_empty():
		_fail("join_lobby failed")
		return
	multiplayer.multiplayer_peer = Condensate.create_peer()
	await multiplayer.connected_to_server
	_log("connected_to_server as peer %d" % multiplayer.get_unique_id())
	if Condensate.lobby.data.get("map") != "arena":
		await Condensate.lobby_data_changed
	_check(Condensate.lobby.data.get("map") == "arena", "lobby data reached the client")
	await get_tree().create_timer(0.3).timeout
	_hello.rpc(role)
	# The other client's hello is relayed through the host.
	while hellos.is_empty():
		await get_tree().process_frame
	_check(hellos[0][1] != 1 and hellos[0][1] != multiplayer.get_unique_id(), "hello from the other client keeps its sender id: %s" % [hellos])
	_done.rpc_id(1)
	await multiplayer.server_disconnected
	_log("server_disconnected after the host left")
	_pass()


@rpc("any_peer", "reliable")
func _hello(from_role: String) -> void:
	hellos.append([from_role, multiplayer.get_remote_sender_id()])


@rpc("any_peer", "reliable")
func _done() -> void:
	done.append(multiplayer.get_remote_sender_id())


func _check(ok: bool, what: String) -> void:
	if not ok:
		_fail(what)
	_log("ok: " + what)


func _log(text: String) -> void:
	print("[%s] %s" % [role, text])


func _pass() -> void:
	print("PASS %s" % role)
	get_tree().quit(0)


func _fail(why: String) -> void:
	print("FAIL %s: %s" % [role, why])
	get_tree().quit(1)
