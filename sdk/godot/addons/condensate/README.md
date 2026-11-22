# Condensate multiplayer for Godot 4

This addon lets players start or join a game with their Condensate friends. Condensate creates the lobbies, sends the invites, and carries the game traffic, so you don't need to run a server or set up port forwarding.

It is pure GDScript and needs Godot 4.3 or later.

## Install

1. Copy `addons/condensate` into your project.
2. Enable **Condensate** under **Project Settings → Plugins**. This adds the `Condensate` autoload.
3. Set **Project Settings → Condensate → Game Id** to your game's id in Condensate (shown in Studio).

To test from the editor, keep the Condensate app running and logged in. The addon finds the app on its own. When players start the game from Condensate, the app passes the connection details to the game, and the Game Id setting is ignored.

## Host and join

```gdscript
func host() -> void:
    if (await Condensate.create_lobby(8)).is_empty():
        return                                       # Condensate.error says why
    multiplayer.multiplayer_peer = Condensate.create_peer()

func join(lobby_id: String) -> void:
    if (await Condensate.join_lobby(lobby_id)).is_empty():
        return
    multiplayer.multiplayer_peer = Condensate.create_peer()

func _ready() -> void:
    # The player clicked Join on a friend, or accepted an invite, in Condensate.
    Condensate.join_requested.connect(join)
```

After that, Godot's high-level multiplayer works as usual: `@rpc`, `MultiplayerSpawner` and `MultiplayerSynchronizer` all work. The host is peer 1, the same as with `ENetMultiplayerPeer`, so code written for ENet works without changes.

The other players' signals behave as follows:
- `multiplayer.peer_connected` fires on the host when someone joins.
- `multiplayer.connected_to_server` fires on the player who joined.
- `multiplayer.server_disconnected` fires for everyone when the host leaves, because the lobby closes.

## API

| | |
|---|---|
| `create_lobby(max_members = 8, invite_only = false, data = {})` | Create a lobby and become its host. Friends see a **Join** button in Condensate unless `invite_only`. |
| `join_lobby(id)` | Join a lobby. |
| `leave_lobby()` | Leave the lobby. If the host leaves, the lobby closes for everyone. |
| `create_peer()` | A `MultiplayerPeer` for the current lobby. Make a new one for each lobby. |
| `invite(username)` | Invite a friend. They get a notification in Condensate with a **Join** button. |
| `get_friends()` | Your friends, with their `status` and a `lobby` of theirs you can join. |
| `list_lobbies()` | Lobbies for this game that you can join. |
| `set_lobby_data(dict)` / `kick(id)` | Host only. |
| `user`, `lobby`, `member_id`, `is_host()`, `is_online()` | Current state. |

**Signals:**
- `connected`, `connection_failed`, `disconnected`
- `lobby_joined`, `lobby_left(reason)`
- `member_joined`, `member_left`
- `lobby_data_changed`
- `join_requested(lobby_id)`
- `error(message)`

Requests are async (`await` them). If one fails, it returns an empty value and emits `error`.

## Notes

- **Routing:** traffic is relayed by the Condensate server over a reliable connection. "Unreliable" packets still arrive in order, but they may be dropped if a player falls behind. Faster routes will come later without any change to your game: direct connections between players, or a UDP relay.
- **Max packet size:** 1 MiB.
- **Testing alone:** run two copies of the game from the editor (**Debug → Customize Run Instances**). Both log in as you, and you're allowed to join your own lobby.
- **Example:** `demo/main.gd` in this repository's `sdk/godot` project is a complete example, with hosting, joining, invites and chat.
