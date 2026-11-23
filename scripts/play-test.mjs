// Protocol test for /api/v1/play (lobbies, invites, relay, kick), run by scripts/play-test.sh.
// Usage: node scripts/play-test.mjs <server url> <game> <alice token> <bob token> <carol token> [game-api.json]
// alice is friends with bob and carol. With a game-api.json (from a running Condensate app logged
// in as alice), also checks games connecting through the app. Needs Node 22+ (global WebSocket).
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [server, game, alice, bob, carol, appFile] = process.argv.slice(2);
const ws = server.replace(/^http/, "ws");
/** Set when the server has Whisper: wait for transcripts and check their text. */
const whisper = !!process.env.CONDENSATE_WHISPER;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function rest(method, path, token, body) {
  const res = await fetch(`${server}/api/v1${path}`, { method, headers: { Authorization: `Bearer ${token}` }, body });
  const buf = new Uint8Array(await res.arrayBuffer());
  const text = new TextDecoder().decode(buf);
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {}
  return { status: res.status, ok: res.ok, json, text, buf };
}

/** A screenshot frame body: [meta length u32][meta JSON][image]. */
function shotBody(meta) {
  const m = new TextEncoder().encode(JSON.stringify(meta));
  const jpeg = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 16, 0x4a, 0x46, 0x49, 0x46, 0, 1, 0xff, 0xd9]);
  const out = new Uint8Array(4 + m.length + jpeg.length);
  new DataView(out.buffer).setUint32(0, m.length, true);
  out.set(m, 4);
  out.set(jpeg, 4 + m.length);
  return out;
}

/** Wait until a recording is finished (and transcribed, with Whisper). */
async function finished(session) {
  for (let i = 0; i < (whisper ? 600 : 20); i++) {
    const r = await rest("GET", `/feedback/${session}`, alice);
    if (r.json && !["recording", "transcribing"].includes(r.json.status)) return r.json;
    await sleep(500);
  }
  throw new Error(`recording ${session} didn't finish`);
}

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

  // Voice and feedback recordings.
  {
    const [h, m, o, watcher] = [play(alice), play(carol), play(bob), hub(bob)];
    await Promise.all([h, m, o, watcher].map((s) => s.ready));
    const lob = await h.request({ op: "create_lobby", max_members: 4 });
    await m.request({ op: "join_lobby", lobby: lob.lobby.id });
    await o.request({ op: "join_lobby", lobby: lob.lobby.id });
    const voiceFrame = (n) => {
      const f = new Uint8Array(6 + 40).fill(n & 255);
      f.fill(0, 0, 6);
      f[5] = 3;
      return f;
    };
    h.sock.send(voiceFrame(1)); // not on voice yet: dropped
    check((await h.request({ op: "voice", on: true })).op === "ok", "alice joins voice");
    check((await m.next((x) => x.op === "member_updated" && x.member.id === 1)).member.voice === true, "the lobby hears alice joined voice");
    await m.request({ op: "voice", on: true });
    h.sock.send(voiceFrame(2));
    const vf = await m.next((x) => x.binary);
    check(new DataView(vf.binary.buffer).getUint32(0, true) === 1 && vf.binary[5] === 3 && vf.binary[6] === 2, "carol hears alice's voice (and nothing from before alice joined voice)");
    check(await o.next((x) => x.binary, 400).then(() => false, () => true), "bob, not on voice, gets no voice frames");

    check((await m.request({ op: "set_feedback", on: true })).op === "error", "only the host can start a feedback recording");
    check((await h.request({ op: "set_feedback", on: true })).op === "ok", "alice starts a feedback recording");
    const fb = await m.next((x) => x.op === "feedback");
    const session = fb.session;
    check(!!session && fb.by.username === "alice", "the lobby hears the recording started");
    const pres = await watcher.next((x) => x.type === "presence" && x.user === "alice" && x.presence.lobby?.recording);
    check(!!pres, "friends see the lobby is recording");
    check((await m.request({ op: "record_me", on: false })).op === "ok", "carol leaves herself out of the recording");
    check(!!(await h.next((x) => x.op === "member_updated" && x.member.id === 2 && x.member.recorded === false)), "the lobby sees carol isn't recorded");
    for (let i = 0; i < 50; i++) {
      h.sock.send(voiceFrame(i));
      m.sock.send(voiceFrame(i));
      await sleep(20);
    }
    const up = await rest("POST", `/feedback/${session}/shots`, bob, shotBody({ scene: "res://level2.tscn", version: "0.4.1", context: { level: 2 } }));
    check(up.ok, "bob saves a screenshot to the recording");
    await sleep(1500); // the stretch of talking ends
    check((await h.request({ op: "set_feedback", on: false })).op === "ok", "alice stops the recording");
    check((await m.next((x) => x.op === "feedback")).session === null, "the lobby hears it stopped");

    const rec = await finished(session);
    check(rec.utterances.length === 1 && rec.utterances[0].username === "alice" && rec.utterances[0].duration_ms >= 900, "alice's speech was saved (and carol's wasn't)");
    check(rec.shots.length === 1 && rec.shots[0].username === "bob" && rec.shots[0].meta.context.level === 2, "the screenshot was saved with the game's details");
    check(rec.status === (whisper ? "failed" : "untranscribed"), `status is ${rec.status} (fake audio)`);
    const md = await rest("GET", `/feedback/${session}/transcript`, alice);
    check(md.text.includes("**Alice:**") && md.text.includes("Bob took a screenshot") && md.text.includes("res://level2.tscn"), "the Markdown transcript has the speech and the screenshot");
    const audio = await rest("GET", `/feedback/${session}/files/${rec.utterances[0].file}`, alice);
    check(new TextDecoder().decode(audio.buf.slice(0, 4)) === "OggS", "the audio is an Ogg file");
    check((await rest("GET", `/games/${game}/feedback`, carol)).status === 403, "only the game's owner sees its recordings");
    check((await rest("GET", `/feedback/${session}`, carol)).status === 404, "others can't open a recording");
    check((await rest("POST", `/feedback/${session}/shots`, bob, shotBody({}))).status === 403, "screenshots are refused after recording stops");
    check((await rest("GET", `/games/${game}/feedback`, alice)).json.some((f) => f.id === session), "alice's recordings list has it");
    check((await rest("DELETE", `/feedback/${session}`, alice)).ok && (await rest("GET", `/feedback/${session}`, alice)).status === 404, "alice deletes it");
    for (const s of [h, m, o, watcher]) s.sock.close();
  }

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
    await sleep(300);

    // Voice through the app. play-test.sh starts it with CONDENSATE_VOICE_TEST_INPUT, a WAV of
    // a few seconds' silence and then speech, played into the "microphone" once voice starts.
    const g = await viaApp(`token=${devToken}&game=${game}`);
    await g.next((m) => m.op === "welcome");
    const lob = await g.request({ op: "create_lobby", max_members: 4 });
    const session = (await g.request({ op: "set_feedback", on: true })).op === "ok" ? (await g.next((m) => m.op === "feedback")).session : null;
    check(!!session, "a game in the app starts a feedback recording");
    const listener = play(bob);
    await listener.ready;
    const joined = await listener.request({ op: "join_lobby", lobby: lob.lobby.id });
    check(joined.lobby.members.find((m) => m.id === 1)?.voice === true, "the app joined voice for alice's game");
    check(await g.next((m) => m.rid >= 2 ** 40, 300).then(() => false, () => true), "the game doesn't see the app's own requests");
    await listener.request({ op: "voice", on: true });
    let frames = 0;
    const until = Date.now() + 10000;
    while (Date.now() < until) {
      const f = await listener.next((m) => m.binary, Math.max(1, until - Date.now())).catch(() => null);
      if (f && f.binary[5] === 3 && new DataView(f.binary.buffer).getUint32(0, true) === 1) frames++;
    }
    check(frames >= 75, `bob hears alice's voice from the app (${frames} frames)`);
    check(await g.next((m) => m.binary, 100).then(() => false, () => true), "the game doesn't receive voice frames");
    g.sock.send(new Uint8Array([0, 0, 0, 0, 0, 4, ...shotBody({ scene: "res://dock.tscn" })]));
    await sleep(1500);
    await g.request({ op: "set_feedback", on: false });
    const rec = await finished(session);
    const said = rec.utterances.filter((u) => u.username === "alice");
    check(said.length >= 1 && said.reduce((t, u) => t + u.duration_ms, 0) >= 2000, `the app's voice was recorded (${said.map((u) => u.duration_ms).join(", ")} ms)`);
    check(rec.shots.length === 1 && rec.shots[0].username === "alice" && rec.shots[0].meta.scene === "res://dock.tscn", "a screenshot from the game was uploaded by the app");
    if (said.length) {
      // The saved audio decodes, and is about as long as the speech.
      const file = join(tmpdir(), `condensate-test-${said[0].file}`);
      writeFileSync(file, (await rest("GET", `/feedback/${session}/files/${said[0].file}`, alice)).buf);
      const secs = Number(execFileSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file]).toString());
      check(secs > 1.5, `the recorded audio decodes (${secs.toFixed(1)} s)`);
    }
    if (whisper) {
      const text = said.map((u) => u.text ?? "").join(" ").toLowerCase();
      check(rec.status === "ready" && text.includes("jump"), `Whisper transcribed it: "${text}"`);
    }
    for (const s of [g, listener]) s.sock.close();
  }
} catch (e) {
  console.log(`FAIL: ${e.message}`);
  failed = true;
}
console.log(failed ? "play protocol test FAILED" : "play protocol test passed");
process.exit(failed ? 1 : 0);
