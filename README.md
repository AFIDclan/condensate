# Condensate

A self-hosted game launcher for sharing games with friends. It has accounts, friends, chat and online status, a library, a store, and fast incremental updates. Publishing is designed to be easy for AI agents.

Friends install the app (Windows or Linux) from `<server>/download/` and create an account with an invite code.

## Layout

| Path | What |
|---|---|
| `crates/core` | Manifest format, content-defined chunking, install/update engine, and API types shared by everything. |
| `crates/sdk` | HTTP client, push/pull/launch, and the login file shared by the CLI and the app. |
| `crates/server` | `condensate-server`: axum + SQLite. Chunks and images are stored on disk; a WebSocket hub handles presence and chat. |
| `crates/cli` | `condensate`: publish, install/update, launch, and manage games. Shipped inside the app's installer. |
| `client/` | Tauri 2 + React desktop app: Library, Store, Friends, Studio, Account. |
| `deploy/` | systemd units (server and nightly maintenance) and deploy scripts. |
| `docs/PUBLISHING.md` | How to publish a game (written for agents too). |

## How updates work

Each build is split into content-defined chunks (FastCDC, about 1 MiB on average). Each chunk is identified by its BLAKE3 hash and stored once on the server, compressed with zstd.

A version's manifest lists each file's chunks. To update, the client:
1. Downloads only the chunks it can't copy from files it already has.
2. Assembles the changed files in `.condensate/staging`.
3. Swaps them into place.

Interrupted updates resume where they stopped. `condensate pull --repair` re-checks every file on disk.

## One login for the app and the CLI

Both the app and the CLI read and write `<config dir>/com.condensate.app/settings.json`, so logging in or out in one does it for both. When the CLI needs a login and has none, it opens the app (which runs as a single instance) and waits for you to log in. Build agents use `CONDENSATE_TOKEN` and `CONDENSATE_SERVER` instead.

## Development

The Rust workspace builds and tests on Linux (the dev server is `oort`):

```sh
deploy/sync.sh                                         # copy source to oort:~/condensate
ssh oort-tunnel 'cd ~/condensate && cargo test && bash scripts/e2e.sh'
deploy/deploy-server.sh                                # release build, install systemd units, restart
```

## Desktop client

```sh
cd client
npm install
npm run tauri dev      # run with hot reload (also builds the bundled CLI)
npm run release        # signed installer + update feed, uploaded to the server
```

`npm run release` builds Windows on this machine and Linux on oort (over ssh), then uploads these to `<server>/download/`:
- **Windows:** `Condensate_<version>_x64-setup.exe`, plus `Condensate-setup.exe` (the link to share)
- **Linux:** the AppImage (`Condensate-x86_64.AppImage`) and a .deb (`Condensate-amd64.deb`); both update themselves
- `latest.json`, which installed apps check to update themselves
- a small download page

Bump `version` in `client/src-tauri/tauri.conf.json` before each release. To build just one platform, use `npm run release -- --only linux` (or `--only windows`). The other platform stays in `latest.json` only if it is on the same version. `npm run release -- --only page` just rebuilds the download page.

On Linux, the .deb puts the `condensate` CLI in `/usr/bin`. The AppImage copies it to `~/.local/bin` each time it starts.

**Linux build host setup (once):** install Rust, unpack Node 22.12 or newer into `~/.local/node`, then run `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev patchelf build-essential file`. The signing key is passed to the remote build over ssh and is never stored there.

- **Signing key:** updates are signed with `~/.tauri/condensate.key`. **Back this file up.** Without it, installed apps can't be updated; they would need reinstalling with a new key.
- **Installer behaviour:** it installs per user with no admin prompt, and puts `condensate` on the user's PATH (`condensate setup add-to-path`).
- **Build requirements:** Rust and Node on every OS. Windows also needs the MSVC C++ build tools and WebView2. Linux needs the WebKitGTK packages listed at https://tauri.app/start/prerequisites/.

## Server operations (on oort)

- **Service:** `systemctl status condensate`. Logs: `journalctl -u condensate -f`.
- **Data:** `/home/sean/condensate-server/data`. It holds:
  - `condensate.db`
  - `chunks/` and `images/`
  - `downloads/` (public: installer and update feed)
  - `backups/`
- **Nightly maintenance (04:30):** `condensate-maintenance.timer` snapshots the database into `backups/`, keeping 14, then deletes chunks and images that nothing references. Run it by hand with `sudo systemctl start condensate-maintenance`.
- **Network:** the server listens on `127.0.0.1:8470`. Public access comes through Tailscale Funnel; WebSockets work through it.
- **Settings:** in `/home/sean/condensate-server/condensate.env`: `CONDENSATE_NAME`, and `CONDENSATE_DIRECT_URL` for the direct fast path later.
- **Admin commands:**
  ```sh
  export CONDENSATE_DATA_DIR=~/condensate-server/data
  ~/condensate-server/bin/condensate-server invite
  ~/condensate-server/bin/condensate-server reset-password <user>
  ~/condensate-server/bin/condensate-server gc --dry-run
  ~/condensate-server/bin/condensate-server backup
  ```
- **Accounts:** every account needs an invite code. The first one comes from `condensate-server invite`, and the first account becomes admin. After that, users invite friends from the app (**Account → Invite friends**) or with `condensate invite create`.
