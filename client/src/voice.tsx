import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errorMessage, VoiceSettings, VoiceState } from "./api";
import { useToast } from "./components";
import { Icon } from "./icons";
import { useSocial } from "./social";

/** The voice room or game lobby you're in and its voice chat, kept up to date by the app. */
export function useVoiceState(): VoiceState | null {
  const [state, setState] = useState<VoiceState | null>(null);
  useEffect(() => {
    api.voiceState().then(setState).catch(() => {});
    const un = listen<VoiceState>("voice-state", (e) => setState(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);
  return state;
}

/** Voice settings, saved as they change. */
function useVoiceSettings() {
  const toast = useToast();
  const [settings, setSettings] = useState<VoiceSettings | null>(null);
  useEffect(() => {
    api.voiceSettings().then(setSettings).catch(() => {});
  }, []);
  const change = useCallback(
    async (patch: Partial<VoiceSettings>) => {
      setSettings((s) => (s ? { ...s, ...patch } : s));
      try {
        // Start from the saved settings: the dock and the settings page both change them.
        const next = { ...(await api.voiceSettings()), ...patch };
        await api.setVoiceSettings(next);
        setSettings(next);
      } catch (e) {
        toast("error", errorMessage(e));
      }
    },
    [toast],
  );
  return [settings, change] as const;
}

const ROOM_CLOSED: Record<string, string> = {
  host_left: "The host ended the voice room",
  kicked: "You were removed from the voice room",
  disconnected: "Lost the connection to the voice room",
};

/**
 * Shown while you're in a voice room or a game's multiplayer lobby: who's there and who's
 * talking, mute and deafen, and (for the host) feedback recording.
 */
export function VoiceDock() {
  const toast = useToast();
  const state = useVoiceState();
  const [, change] = useVoiceSettings();
  const [open, setOpen] = useState(true);
  const [inviting, setInviting] = useState(false);

  useEffect(() => {
    const a = listen<string>("voice-error", (e) => toast("error", e.payload));
    const b = listen<{ session: string | null; by: string; game: string | null }>("feedback-changed", (e) =>
      toast(
        "info",
        e.payload.session
          ? `${e.payload.by} started recording ${e.payload.game ?? "this game"} for feedback`
          : `${e.payload.by} stopped the feedback recording`,
      ),
    );
    const c = listen<string>("room-closed", (e) => toast("info", ROOM_CLOSED[e.payload] ?? "The voice room closed"));
    return () => {
      a.then((f) => f());
      b.then((f) => f());
      c.then((f) => f());
    };
  }, [toast]);

  if (!state?.lobby) return null;
  const me = state.members.find((m) => m.you);
  const recording = !!state.feedback;
  const { muted, deafened, room } = state;
  const recordingWhat = state.feedback_game?.title ?? state.game?.title ?? "this game";

  const run = (f: () => Promise<void>) => f().catch((e) => toast("error", errorMessage(e)));

  return (
    <div className={`voice-dock ${recording ? "recording" : ""}`}>
      <button className="voice-head" onClick={() => setOpen((o) => !o)}>
        <span className={`voice-dot ${state.connected ? "on" : ""}`} />
        <span className="voice-title">
          <b>{room ? "Voice room" : (state.game?.title ?? "Game")}</b>
          <span>
            {recording
              ? `Recording ${recordingWhat} for feedback`
              : state.connected
                ? `Voice · ${state.members.length} in ${room ? "room" : "lobby"}`
                : "Lobby"}
          </span>
        </span>
        <Icon name="chevron" size={14} className={open ? "voice-chev open" : "voice-chev"} />
      </button>
      {open && (
        <>
          <div className="voice-members">
            {state.members.map((m) => (
              <div key={m.id} className={`voice-member ${m.speaking ? "speaking" : ""} ${m.voice ? "" : "no-voice"}`} title={m.voice ? undefined : "Not connected to voice"}>
                <span className="voice-avatar">{m.display_name.slice(0, 1).toUpperCase()}</span>
                <span className="voice-name">
                  {m.display_name}
                  {m.you && <span className="dim"> (you)</span>}
                  {m.id === 1 && <span className="dim"> · host</span>}
                </span>
                {recording && !m.recorded && <span className="voice-tag" title="Not included in the recording">not recorded</span>}
              </div>
            ))}
          </div>
          {room && inviting && <InviteList members={state.members.map((m) => m.username)} onDone={() => setInviting(false)} />}
          {recording && me && (
            <div className="voice-rec">
              <span>
                {me.recorded
                  ? `Your voice and F12 screenshots are saved for the developer of ${recordingWhat}.`
                  : "Your voice is left out of this recording."}
              </span>
              <button className="text-btn small" onClick={() => run(() => api.setRecordMe(!me.recorded))}>
                {me.recorded ? "Leave me out" : "Include me"}
              </button>
            </div>
          )}
          {state.problems.length > 0 && <div className="voice-problem small">{state.problems.join(" · ")}</div>}
          <div className="voice-controls">
            <button className={`voice-btn ${muted ? "off" : ""}`} onClick={() => change({ muted: !muted })} title={muted ? "Unmute" : "Mute"}>
              <Icon name={muted ? "mic-off" : "mic"} size={17} />
            </button>
            <button className={`voice-btn ${deafened ? "off" : ""}`} onClick={() => change({ deafened: !deafened })} title={deafened ? "Undeafen" : "Deafen"}>
              <Icon name={deafened ? "headphones-off" : "headphones"} size={17} />
            </button>
            {room && (
              <>
                <button className={`voice-btn ${inviting ? "active" : ""}`} onClick={() => setInviting((i) => !i)} title="Invite friends">
                  <Icon name="plus" size={17} />
                </button>
                <button className="voice-btn" onClick={() => run(api.roomLeave)} title={state.host ? "Leave and end the voice room" : "Leave the voice room"}>
                  <Icon name="x" size={17} />
                </button>
              </>
            )}
            <span className="grow" />
            {state.host ? (
              <button
                className={`rec-btn ${recording ? "on" : ""}`}
                disabled={room && !recording && !state.record_game}
                title={
                  recording
                    ? undefined
                    : room
                      ? state.record_game
                        ? `Record everyone's voice and F12 screenshots for the developer of ${state.record_game.title}`
                        : "Start a game first, then record feedback for it"
                      : "Record everyone's voice and F12 screenshots for the game's developer"
                }
                onClick={() => run(() => api.setFeedback(!recording))}
              >
                <span className="rec-dot" />
                {recording ? "Stop recording" : "Record feedback"}
              </button>
            ) : (
              recording && (
                <span className="rec-btn on static">
                  <span className="rec-dot" /> REC
                </span>
              )
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** Online friends who aren't in the room yet, to invite. */
function InviteList({ members, onDone }: { members: string[]; onDone: () => void }) {
  const toast = useToast();
  const social = useSocial();
  const [sent, setSent] = useState<string[]>([]);
  const candidates = (social.friends?.friends ?? []).filter((f) => f.presence.status !== "offline" && !members.includes(f.user.username));
  return (
    <div className="voice-invite">
      {candidates.length === 0 && <span className="dim small">No friends online to invite.</span>}
      {candidates.map((f) => (
        <button
          key={f.user.username}
          className="voice-invite-row"
          disabled={sent.includes(f.user.username)}
          onClick={() =>
            api.roomInvite(f.user.username).then(
              () => setSent((s) => [...s, f.user.username]),
              (e) => toast("error", errorMessage(e)),
            )
          }
        >
          <span className="voice-avatar">{f.user.display_name.slice(0, 1).toUpperCase()}</span>
          <span className="voice-name">{f.user.display_name}</span>
          <span className="voice-tag">{sent.includes(f.user.username) ? "Invited" : "Invite"}</span>
        </button>
      ))}
      <button className="text-btn small" onClick={onDone}>
        Done
      </button>
    </div>
  );
}

/** Start a voice room, or show the one you're in (Friends page). */
export function VoiceRoomCard() {
  const toast = useToast();
  const state = useVoiceState();
  const [busy, setBusy] = useState(false);
  if (state?.room && state.lobby) {
    return (
      <div className="room-card">
        <div className="room-card-text">
          <b>You're in a voice room</b>
          <span className="dim small">
            {state.members.length} {state.members.length === 1 ? "person" : "people"} · invite friends with the + in the voice panel
          </span>
        </div>
        <button className="text-btn small danger" onClick={() => api.roomLeave().catch(() => {})}>
          Leave
        </button>
      </div>
    );
  }
  return (
    <div className="room-card">
      <div className="room-card-text">
        <b>Voice room</b>
        <span className="dim small">Talk with friends before you pick a game. It keeps going while you play.</span>
      </div>
      <button
        className="plain-btn outline small-btn"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            await api.roomStart();
          } catch (e) {
            toast("error", errorMessage(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? "Starting…" : "Start"}
      </button>
    </div>
  );
}

/** Account page: devices, sensitivity, noise suppression, volume, and a microphone test. */
export function VoiceSettingsCard() {
  const [settings, change] = useVoiceSettings();
  const [devices, setDevices] = useState<{ inputs: string[]; outputs: string[] } | null>(null);
  const [testing, setTesting] = useState(false);
  const state = useVoiceState();
  const testingRef = useRef(false);

  useEffect(() => {
    api.voiceDevices().then(setDevices).catch(() => {});
    return () => {
      if (testingRef.current) api.micTest(false).catch(() => {});
    };
  }, []);

  if (!settings) return null;
  const level = state?.level ?? null;
  const live = testing || !!state?.connected;
  // Map -80..0 dB to the meter's width.
  const pct = (db: number) => Math.max(0, Math.min(100, ((db + 80) / 80) * 100));

  return (
    <section className="card">
      <h2>Voice chat</h2>
      <p className="dim">
        Voice chat starts on its own when you're in a voice room (start one on the Friends page) or a game's multiplayer lobby, and only sends
        while you talk. Headphones work best: with speakers, your friends may hear themselves echo.
      </p>
      <label className="field">
        Microphone
        <select value={settings.input ?? ""} onChange={(e) => change({ input: e.target.value || null })}>
          <option value="">System default</option>
          {devices?.inputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        Speakers or headphones
        <select value={settings.output ?? ""} onChange={(e) => change({ output: e.target.value || null })}>
          <option value="">System default</option>
          {devices?.outputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
      <label className="check">
        <input type="checkbox" checked={settings.auto_sensitivity} onChange={(e) => change({ auto_sensitivity: e.target.checked })} />
        Detect when I'm talking automatically
      </label>
      {!settings.auto_sensitivity && (
        <label className="field">
          Send when louder than {Math.round(settings.threshold_db)} dB
          <input type="range" min={-80} max={0} step={1} value={settings.threshold_db} onChange={(e) => change({ threshold_db: Number(e.target.value) })} />
        </label>
      )}
      <div className="mic-meter" title={live ? `${level ?? -100} dB` : "Test your microphone to see its level"}>
        <div className={`mic-level ${state?.talking ? "talking" : ""}`} style={{ width: `${live && level != null ? pct(level) : 0}%` }} />
        {!settings.auto_sensitivity && <div className="mic-threshold" style={{ left: `${pct(settings.threshold_db)}%` }} />}
      </div>
      <div className="row">
        <button
          className="plain-btn outline"
          disabled={!!state?.connected}
          onClick={async () => {
            const on = !testing;
            setTesting(on);
            testingRef.current = on;
            await api.micTest(on).catch(() => {});
          }}
        >
          {testing ? "Stop test" : "Test microphone"}
        </button>
        {live && <span className="dim small">{state?.talking ? "Sending: you're talking" : "Not sending"}</span>}
      </div>
      <label className="check">
        <input type="checkbox" checked={settings.noise_suppression} onChange={(e) => change({ noise_suppression: e.target.checked })} />
        Noise suppression (removes fans, keyboards and background hum)
      </label>
      <label className="field">
        Volume {Math.round(settings.volume * 100)}%
        <input type="range" min={0} max={2} step={0.05} value={settings.volume} onChange={(e) => change({ volume: Number(e.target.value) })} />
      </label>
      {state?.problems && state.problems.length > 0 && <p className="voice-problem small">{state.problems.join(" · ")}</p>}
    </section>
  );
}
