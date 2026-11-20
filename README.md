# Condensate

A self-hosted game launcher for sharing games with friends. It has accounts, a library, a store, and fast incremental updates. Publishing is designed to be easy for AI agents.

## Layout

| Path | What |
|---|---|
| `crates/core` | Manifest format, content-defined chunking, install/update engine, and API types shared by everything. |
| `crates/sdk` | HTTP client plus push, pull and launch logic, used by the CLI and the desktop client. |
| `crates/server` | `condensate-server`: axum + SQLite, with chunks stored on disk. |
| `crates/cli` | `condensate`: log in, publish, install/update, launch. |
| `client/` | Tauri 2 + React desktop app (Library, Store, Account). |
| `deploy/` | systemd unit and deploy scripts for the server. |
| `docs/PUBLISHING.md` | How to publish a game (written for agents too). |

## How updates work

Each build is split into content-defined chunks (FastCDC, about 1 MiB on average). Each chunk is identified by its BLAKE3 hash and stored once on the server, compressed with zstd.

A version's manifest lists each file's chunks. To update, the client:
1. Downloads only the chunks it can't copy from files it already has.
2. Assembles the changed files in `.condensate/staging`.
3. Swaps them into place.

Interrupted updates resume where they stopped. `condensate pull --repair` re-checks every file on disk.

## Development

The workspace builds and tests on Linux (the dev server is `oort`):

```sh
deploy/sync.sh                                         # copy source to oort:~/condensate
ssh oort-tunnel 'cd ~/condensate && cargo test && bash scripts/e2e.sh'
deploy/deploy-server.sh                                # release build + restart the systemd service
```

## Desktop client

```sh
cd client
npm install
npm run tauri dev      # run with hot reload
npm run tauri build    # Windows installer in target/release/bundle/nsis/
```

You need Rust and Node on every OS. Windows also needs the MSVC C++ build tools and WebView2. Linux also needs the WebKitGTK packages listed at https://tauri.app/start/prerequisites/.
Games install into the app's local data folder (on Windows: `%LOCALAPPDATA%\com.condensate.app\games`).

## Server operations (on oort)

- Service: `systemctl status condensate`. Logs: `journalctl -u condensate -f`.
- Data: `/home/sean/condensate-server/data`, which holds `condensate.db` and `chunks/`. Back up the `.db` file.
- Listens on `127.0.0.1:8470`. Public access comes through Tailscale Funnel.
- Settings go in `/home/sean/condensate-server/condensate.env`:
  `CONDENSATE_NAME`, and `CONDENSATE_DIRECT_URL` for the direct fast path later.
- Admin commands:
  ```sh
  CONDENSATE_DATA_DIR=~/condensate-server/data ~/condensate-server/bin/condensate-server invite
  CONDENSATE_DATA_DIR=~/condensate-server/data ~/condensate-server/bin/condensate-server reset-password <user>
  ```
- Every account needs an invite code. Make the first one with `condensate-server invite` on the server. The first account becomes admin, and after that users invite friends with `condensate invite create`.
