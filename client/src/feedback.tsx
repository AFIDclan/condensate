import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, FeedbackShot, FeedbackSummary, FeedbackTimeline, FeedbackUtterance, timeAgo } from "./api";
import { Spinner, useToast } from "./components";

const STATUS: Record<FeedbackSummary["status"], string> = {
  recording: "Recording",
  transcribing: "Transcribing",
  ready: "Ready",
  untranscribed: "Not transcribed",
  failed: "Failed",
};

function clock(ms: number) {
  const s = Math.floor(ms / 1000);
  const mm = String(Math.floor(s / 60) % 60).padStart(2, "0");
  const ss = String(s % 60).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`;
}

function minutes(f: FeedbackSummary) {
  if (!f.ended_at) return "still going";
  const m = Math.round((f.ended_at - f.started_at) / 60);
  return m < 1 ? "under a minute" : `${m} min`;
}

/** Studio: a game's feedback recordings (playtest voice transcripts and screenshots). */
export function FeedbackCard({ slug }: { slug: string }) {
  const toast = useToast();
  const [list, setList] = useState<FeedbackSummary[] | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);

  const load = useCallback(() => {
    api.feedbackList(slug).then(setList).catch((e) => toast("error", errorMessage(e)));
  }, [slug, toast]);
  useEffect(load, [load]);
  // Recordings change state on their own (recording, then transcribing, then ready).
  const busy = list?.some((f) => f.status === "recording" || f.status === "transcribing");
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(load, 8000);
    return () => clearInterval(t);
  }, [busy, load]);

  const run = (f: () => Promise<unknown>) => f().catch((e) => toast("error", errorMessage(e)));

  return (
    <section className="card">
      <h2>Playtest feedback</h2>
      <p className="dim">
        When you host a lobby, press <b>Record feedback</b> in the voice panel. Everyone's voice chat is saved and transcribed per player, and
        players press F12 to screenshot moments they want to point out. Hand a recording to your AI agent and it can read what people said
        and saw.
      </p>
      {!list && <Spinner />}
      {list?.length === 0 && <p className="dim small">No recordings yet.</p>}
      <div className="feedback-list">
        {list?.map((f) => (
          <div key={f.id} className={`feedback-row ${openId === f.id ? "open" : ""}`}>
            <div>
              <b>{new Date(f.started_at * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</b>
              <div className="feedback-meta">
                {minutes(f)} · {f.players.length ? f.players.join(", ") : `hosted by ${f.host}`} · {f.utterances} lines · {f.shots} screenshots
                {f.version ? ` · v${f.version}` : ""}
              </div>
              {f.note && f.status !== "ready" && <div className="feedback-meta">{f.note}</div>}
            </div>
            <span className={`status-chip ${f.status}`}>{STATUS[f.status] ?? f.status}</span>
            <div className="feedback-actions">
              <button className="text-btn small" onClick={() => setOpenId(openId === f.id ? null : f.id)}>
                {openId === f.id ? "Hide" : "Read"}
              </button>
              <button
                className="text-btn small"
                onClick={async () => {
                  const command = `condensate feedback pull ${slug} ${f.id}`;
                  await navigator.clipboard.writeText(command).catch(() => {});
                  toast("info", `Copied "${command}". Ask your agent to run it and read the transcript.md it saves.`);
                }}
              >
                Copy command for AI
              </button>
              <button
                className="text-btn small"
                onClick={() =>
                  run(async () => {
                    const dir = await open({ directory: true, title: "Save the recording into…" });
                    if (typeof dir !== "string") return;
                    const saved = await api.exportFeedback(f.id, dir);
                    toast("info", `Saved to ${saved}`);
                  })
                }
              >
                Save to folder…
              </button>
              {(f.status === "failed" || f.status === "untranscribed") && (
                <button
                  className="text-btn small"
                  onClick={() =>
                    run(async () => {
                      await api.retranscribeFeedback(f.id);
                      load();
                    })
                  }
                >
                  Transcribe again
                </button>
              )}
              {f.status !== "recording" && (
                <button
                  className="text-btn small danger"
                  onClick={() =>
                    run(async () => {
                      if (!confirm("Delete this recording, its audio and screenshots?")) return;
                      await api.deleteFeedback(f.id);
                      load();
                    })
                  }
                >
                  Delete
                </button>
              )}
            </div>
            {openId === f.id && <Timeline id={f.id} status={f.status} />}
          </div>
        ))}
      </div>
    </section>
  );
}

type Item = { at: number; said?: FeedbackUtterance; shot?: FeedbackShot };

function Timeline({ id, status }: { id: string; status: string }) {
  const toast = useToast();
  const [data, setData] = useState<FeedbackTimeline | null>(null);
  const [zoom, setZoom] = useState<string | null>(null);
  useEffect(() => {
    api.feedbackDetail(id).then(setData).catch((e) => toast("error", errorMessage(e)));
  }, [id, status, toast]);
  if (!data)
    return (
      <div className="timeline">
        <Spinner />
      </div>
    );
  const items: Item[] = [...data.utterances.map((u) => ({ at: u.at_ms, said: u })), ...data.shots.map((s) => ({ at: s.at_ms, shot: s }))].sort(
    (a, b) => a.at - b.at,
  );
  return (
    <div className="timeline">
      {items.length === 0 && <p className="dim small">Nothing was said and no screenshots were taken.</p>}
      {items.map((it) =>
        it.said ? (
          <div key={`u${it.said.id}`} className="tl-line">
            <span className="tl-time">{clock(it.at)}</span>
            <span>
              <span className="tl-who">{it.said.display_name}</span>
              {it.said.text === null ? (
                <span className="dim">(not transcribed yet)</span>
              ) : it.said.text === "" ? (
                <span className="dim">(nothing intelligible)</span>
              ) : (
                it.said.text
              )}
            </span>
          </div>
        ) : (
          <div key={`s${it.shot!.id}`} className="tl-line tl-shot">
            <span className="tl-time">{clock(it.at)}</span>
            <span>
              <span className="tl-who">{it.shot!.display_name}</span>
              <span className="dim">took a screenshot</span>
              <ShotAbout shot={it.shot!} />
              <ShotImage id={id} file={it.shot!.file} onZoom={setZoom} />
            </span>
          </div>
        ),
      )}
      <p className="dim small">Recorded {timeAgo(data.started_at)}.</p>
      {zoom && (
        <div className="shot-zoom" onClick={() => setZoom(null)}>
          <img src={zoom} alt="" />
        </div>
      )}
    </div>
  );
}

function ShotAbout({ shot }: { shot: FeedbackShot }) {
  const parts = [shot.meta.note && `"${shot.meta.note}"`, shot.meta.scene, shot.meta.version && `v${shot.meta.version}`].filter(Boolean);
  const context = shot.meta.context && typeof shot.meta.context === "object" ? JSON.stringify(shot.meta.context) : null;
  if (!parts.length && !context) return null;
  return <div className="tl-about">{[...parts, context].filter(Boolean).join(" · ")}</div>;
}

function ShotImage({ id, file, onZoom }: { id: string; file: string; onZoom: (url: string) => void }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let made: string | null = null;
    api
      .feedbackFile(id, file)
      .then((bytes) => {
        made = URL.createObjectURL(new Blob([bytes], { type: file.endsWith(".png") ? "image/png" : "image/jpeg" }));
        setUrl(made);
      })
      .catch(() => {});
    return () => {
      if (made) URL.revokeObjectURL(made);
    };
  }, [id, file]);
  return url ? <img src={url} alt="" onClick={() => onZoom(url)} /> : <div className="dim small">Loading screenshot…</div>;
}
