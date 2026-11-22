# Publishing a game to Condensate

This guide is written for both people and AI coding agents. Publishing is a single command.

## One-time setup (a person does this)

The Condensate app installs the `condensate` command and shares its login with it. If a command needs a login and you aren't logged in, it opens the app so you can log in there.

1. Create the game: in the app, go to **Studio → New game**. Or from a terminal (the game id is lowercase letters, digits and dashes):
   ```sh
   condensate game create my-game --title "My Game" --summary "One line for the store"
   ```
2. Create a publish token for the agent: **Studio → your game → Publish tokens → New token**, or run `condensate token create --name "claude agent" --game my-game`. A publish token can only publish that one game. It can't manage the account.
3. Give the agent these environment variables:
   ```sh
   CONDENSATE_SERVER=https://<server address>
   CONDENSATE_TOKEN=cdp_...
   ```

## condensate.toml (keep in the game's repo)

```toml
game = "my-game"
cover = "art/cover.png"       # optional store poster, portrait 2:3 (e.g. 600x900)
banner = "art/banner.png"     # optional store banner, landscape 16:9 (e.g. 1920x1080)

[[build]]
platform = "windows-x64"      # or linux-x64
dir = "export/windows"        # folder with the exported build, relative to this file
exe = "MyGame.exe"            # path to the executable inside that folder

[[build]]
platform = "linux-x64"
dir = "export/linux"
exe = "MyGame.x86_64"
# args = ["--fullscreen"]     # optional launch arguments
# workdir = "."               # optional; defaults to the executable's folder
```

You only need the platforms you actually build. A version that ships only Windows leaves the latest Linux build available.

## Every release

1. Export or build the game into the `dir` folders.
2. Push with a short changelog:
   ```sh
   condensate push -m "Added the boss fight; fixed jumping through walls"
   ```

Rules for the changelog:
- One or two plain sentences that a player would understand.
- Required. The push fails without it.

The version number is optional. When you leave it out, the last number goes up by one (0.4.2 → 0.4.3). Pass `--version 1.0.0` to choose one yourself.

What happens:
- Every file is split into chunks.
- Only chunks the server hasn't seen before are uploaded, so small changes upload in seconds even for large games.
- Players' clients then download only the changed chunks.

Useful flags:
- `--only linux-x64` pushes one platform from the project file.
- `--json` prints machine-readable output.
- Without a project file: `condensate push --dir build --game my-game --platform linux-x64 --exe run.sh -m "..."`.

## Artwork

When `condensate.toml` names a `cover` or `banner`, `condensate push` uploads it if it changed. To set artwork without pushing a build:
```sh
condensate game edit my-game --cover art/cover.png --banner art/banner.png
```
You can also upload artwork in the app under **Studio → Artwork**. Images can be PNG, JPEG, GIF or WebP, up to 8 MB each.

## Removing a bad version

```sh
condensate game delete-version my-game 0.4.3
```
Or use **Studio → Versions**. Players are then offered the newest remaining version.

## Multiplayer

Condensate provides lobbies, friend invites and a relay for game traffic, so a game needs no server of its own.

**Godot 4.3+:**
1. Copy `sdk/godot/addons/condensate` from the Condensate repository into the project and enable the plugin.
2. Set **Project Settings → Condensate → Game Id** to the game's id.
3. To host, call `await Condensate.create_lobby()`. To join, call `await Condensate.join_lobby(id)`.
4. Then set `multiplayer.multiplayer_peer = Condensate.create_peer()`.
5. Connect `Condensate.join_requested` to a function that joins that lobby. It fires when the player clicks **Join** on a friend in Condensate, or accepts an invite.

The host is peer 1, as with ENet, so RPCs, `MultiplayerSpawner` and `MultiplayerSynchronizer` work unchanged. See `sdk/godot/addons/condensate/README.md` for the full API.

**Other engines:** the protocol is a WebSocket carrying JSON messages and binary packets; see `crates/core/src/play.rs`. When Condensate launches a game, it sets three environment variables:
- `CONDENSATE_API`
- `CONDENSATE_TOKEN`
- `CONDENSATE_GAME`

## Checking the result

```sh
condensate versions my-game       # version history with changelogs
condensate game info my-game
```

## Common errors

| Message | Fix |
|---|---|
| `launch executable "X" is not part of the build` | `exe` must be a path inside `dir`, using `/` separators. |
| `version X already exists` | Leave out `--version`, or choose a new one. |
| `you can't publish this game` | The token belongs to another game or user. |
| `path contains a character that is not allowed` | File names can't use `: * ? " < > |` or end with a dot or space, because Windows can't store them. |
