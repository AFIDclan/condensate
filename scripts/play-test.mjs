// Protocol test for /api/v1/play (lobbies, invites, relay, kick), run by scripts/play-test.sh.
// Usage: node scripts/play-test.mjs <server url> <game> <alice token> <bob token> <carol token> [game-api.json]
// alice is friends with bob and carol. With a game-api.json (from a running Condensate app logged
// in as alice), also checks games connecting through the app. Needs Node 22+ (global WebSocket).
import { readFileSync } from "node:fs";

const [server, game, alice, bob, carol, appFile] = process.argv.slice(2);
const ws = server.replace(/^http/, "ws");

let failed = false;
function check(ok, what) {
  console.log(`${ok ? "ok" : "FAIL"}: ${what}`);
  if (!ok) failed = true;
}

/** A socket with a queue of received messages and a way to wait for the next matching one. */
function open(url) {
  const sock = new WebSocket(url);
  sock.binaryType = "arraybuffer";
  const queue = [];
  const waiters = [];
  sock.onmessage = (e) => {
    const msg = typeof e.data === "string" ? JSON.parse(e.data) : { binary: new Uint8Array(e.data) };
    const i = waiters.findIndex((w) => w.match(msg));
    if (i >= 0) waiters.splice(i, 1)[0].resolve(msg);
    else queue.push(msg);
  };
  const api = {
    sock,
    ready: new Promise((res, rej) => {
      sock.onopen = res;
      sock.onerror = () => rej(new Error(`could not open ${url}`));
    }),
    send: (obj) => sock.send(JSON.stringify(obj)),
    /** Forget messages received so far. */
    drain: () => void queue.splice(0),
    next(match = () => true, ms = 3000) {
      const i = queue.findIndex(match);
      if (i >= 0) return Promise.resolve(queue.splice(i, 1)[0]);
      return new Promise((resolve, reject) => {
        const w = { match, resolve };
        waiters.push(w);
        setTimeout(() => {
          const j = waiters.indexOf(w);
          if (j >= 0) waiters.splice(j, 1), reject(new Error("timed out waiting for a message"));
        }, ms);
      });
    },
    /** Send a command with a request id and wait for its reply. */
    async request(obj) {
      const rid = Math.floor(Math.random() * 1e9);
      api.send({ ...obj, rid });
      return api.next((m) => m.rid === rid);
    },
  };
  return api;
}

const play = (token) => open(`${ws}/api/v1/play?token=${token}&game=${game}`);
const hub = (token) => open(`${ws}/api/v1/ws?token=${token}`);
const packet = (peer, payload) => {
  const out = new Uint8Array(6 + payload.length);
  new DataView(out.buffer).setUint32(0, peer, true);
  out[4] = 0;
  out[5] = 2; // reliable
  out.set(payload, 6);
  return out;
};

try {
  const [a, b, c, carolHub, bobHub] = [play(alice), play(bob), play(carol), hub(carol), hub(bob)];
  await Promise.all([a, b, c, carolHub, bobHub].map((s) => s.ready));
  const welcome = await a.next((m) => m.op === "welcome");
  check(welcome.you.username === "alice" && welcome.game.slug === game, "welcome names the user and game");

  // An unknown game is refused before the socket opens.
  const bad = new WebSocket(`${ws}/api/v1/play?token=${alice}&game=no-such-game`);
  check(await new Promise((r) => ((bad.onopen = () => r(false)), (bad.onerror = () => r(true)))), "unknown game is refused");

  // Friends lobby: shows up in bob's presence.
  const friendsLobby = await a.request({ op: "create_lobby", max_members: 4 });
  check(friendsLobby.op === "lobby" && friendsLobby.you === 1, "alice creates a lobby as member 1");
  const presence = await bobHub.next((m) => m.type === "presence" && m.user === "alice" && m.presence.lobby);
  check(presence.presence.lobby.id === friendsLobby.lobby.id && presence.presence.lobby.members === 1, "bob sees alice's joinable lobby in her presence");
  const friends = await b.request({ op: "list_friends" });
  check(friends.friends.some((f) => f.username === "alice" && f.lobby === friendsLobby.lobby.id), "list_friends shows alice's lobby");

  // Invite-only lobby (replaces the first): carol can't see or join it until invited.
  bobHub.drain();
  const lobby = await a.request({ op: "create_lobby", max_members: 3, visibility: "invite_only", data: { map: "dock" } });
  const lobbyId = lobby.lobby.id;
  check(lobby.op === "lobby" && lobbyId !== friendsLobby.lobby.id, "alice creates an invite-only lobby");
  const cleared = await bobHub.next((m) => m.type === "presence" && m.user === "alice" && !m.presence.lobby);
  check(!!cleared, "an invite-only lobby isn't shown in presence");
  check((await c.request({ op: "list_lobbies" })).lobbies.length === 0, "carol can't list it");
  const refused = await c.request({ op: "join_lobby", lobby: lobbyId });
  check(refused.op === "error" && /invite/.test(refused.message), "carol can't join without an invite");

  check((await a.request({ op: "invite", user: "carol" })).op === "ok", "alice invites carol");
  const invite = await carolHub.next((m) => m.type === "lobby_invite");
  check(invite.from === "alice" && invite.lobby.id === lobbyId && invite.lobby.game.slug === game, "carol's app gets the invite");
  check((await c.request({ op: "list_lobbies" })).lobbies.some((l) => l.id === lobbyId && l.host.username === "alice"), "carol can list it now");
  const joined = await c.request({ op: "join_lobby", lobby: lobbyId });
  check(joined.op === "lobby" && joined.you === 2 && joined.lobby.data.map === "dock", "carol joins as member 2 and sees the lobby data");
  const mj = await a.next((m) => m.op === "member_joined");
  check(mj.member.id === 2 && mj.member.username === "carol", "alice hears that carol joined");
  const bobRefused = await b.request({ op: "join_lobby", lobby: lobbyId });
  check(bobRefused.op === "error", "bob (not invited) can't join the invite-only lobby");

  // Relay: alice -> 2 arrives from 1; carol -> everyone arrives at alice from 2.
  a.sock.send(packet(2, new TextEncoder().encode("hi carol")));
  const p1 = await c.next((m) => m.binary);
  const v1 = new DataView(p1.binary.buffer);
  check(v1.getUint32(0, true) === 1 && new TextDecoder().decode(p1.binary.slice(6)) === "hi carol", "packet to member 2 arrives from member 1");
  c.sock.send(packet(0, new Uint8Array([1, 2, 3])));
  const p2 = await a.next((m) => m.binary);
  check(new DataView(p2.binary.buffer).getUint32(0, true) === 2 && p2.binary.length === 9, "broadcast from carol arrives at alice from member 2");

  // Non-hosts can't change data or kick.
  check((await c.request({ op: "set_lobby_data", data: { map: "x" } })).op === "error", "only the host can set lobby data");
  check((await a.request({ op: "set_lobby_data", data: { map: "pier" } })).op === "ok", "host sets lobby data");
  check((await c.next((m) => m.op === "lobby_data")).data.map === "pier", "carol gets the new lobby data");

  // Kick, then the host leaving closes the lobby.
  check((await a.request({ op: "kick", member: 2 })).op === "ok", "host kicks carol");
  check((await c.next((m) => m.op === "lobby_closed")).reason === "kicked", "carol is told she was kicked");
  check((await a.next((m) => m.op === "member_left")).member === 2, "alice hears carol left");
  check((await c.request({ op: "join_lobby", lobby: lobbyId })).op === "lobby", "carol (still invited) rejoins");
  await a.next((m) => m.op === "member_joined");
  a.sock.close();
  check((await c.next((m) => m.op === "lobby_closed")).reason === "host_left", "lobby closes when the host disconnects");

  for (const s of [b, c, carolHub, bobHub]) s.sock.close();

  if (appFile) {
    // Games connecting through the app (client/src-tauri/src/game_api.rs).
    const { api: appApi, token: devToken } = JSON.parse(readFileSync(appFile, "utf8"));
    const viaApp = async (query) => {
      const s = open(`${appApi}?${query}`);
      await s.ready;
      return s;
    };
    const g1 = await viaApp(`token=${devToken}&game=${game}`);
    check((await g1.next((m) => m.op === "welcome")).you.username === "alice", "a game connected through the app is logged in as the app's user");
    const hosted = await g1.request({ op: "create_lobby", max_members: 2 });
    const g2 = await viaApp(`token=${devToken}&game=${game}`);
    await g2.next((m) => m.op === "welcome");
    const j = await g2.request({ op: "join_lobby", lobby: hosted.lobby.id });
    check(j.op === "lobby" && j.you === 2, "a second copy joins its own user's lobby through the app");
    g1.sock.send(packet(2, new TextEncoder().encode("via app")));
    const p = await g2.next((m) => m.binary);
    check(new DataView(p.binary.buffer).getUint32(0, true) === 1 && new TextDecoder().decode(p.binary.slice(6)) === "via app", "packets pass through the app both ways");
    const badToken = await viaApp(`token=nope&game=${game}`);
    check(/token/.test((await badToken.next((m) => m.op === "error")).message), "the app refuses a wrong token");
    const badGame = await viaApp(`token=${devToken}&game=no-such-game`);
    check(/no game called/.test((await badGame.next((m) => m.op === "error")).message), "the app reports an unknown game id");
    for (const s of [g1, g2, badToken, badGame]) s.sock.close();
  }
} catch (e) {
  console.log(`FAIL: ${e.message}`);
  failed = true;
}
console.log(failed ? "play protocol test FAILED" : "play protocol test passed");
process.exit(failed ? 1 : 0);
