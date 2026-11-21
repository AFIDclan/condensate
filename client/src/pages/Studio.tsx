import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, GameChanges, GameDetail, GameSummary, platformLabel, timeAgo, TokenInfo } from "../api";
import { GameImage } from "../art";
import { pickAndUploadImage, Spinner, useToast } from "../components";
import { Icon } from "../icons";

/** Manage the games you publish: store page, artwork, versions and publish tokens. */
export default function Studio({ server, openStorePage }: { server: string; openStorePage: (slug: string) => void }) {
  const toast = useToast();
  const [games, setGames] = useState<GameSummary[] | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    try {
      const g = await api.myGames();
      setGames(g);
      setSel((s) => s ?? g[0]?.slug ?? null);
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }, [toast]);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <div className="studio">
      <aside className="studio-side">
        <h3 className="side-title">Your games</h3>
        {!games && <Spinner />}
        {games?.map((g) => (
          <button key={g.slug} className={`studio-game ${sel === g.slug ? "on" : ""}`} onClick={() => setSel(g.slug)}>
            <span className="studio-thumb">
              <GameImage game={g} kind="cover" />
            </span>
            <span className="person-text">
              <b>{g.title}</b>
              <span>{g.latest_version ? `v${g.latest_version}` : "unreleased"} · {g.visibility}</span>
            </span>
          </button>
        ))}
        {creating ? (
          <NewGame
            onCancel={() => setCreating(false)}
            onCreated={(slug) => {
              setCreating(false);
              setSel(slug);
              load();
            }}
          />
        ) : (
          <button className="plain-btn outline new-game" onClick={() => setCreating(true)}>
            <Icon name="plus" size={15} /> New game
          </button>
        )}
      </aside>
      <div className="studio-main">
        {sel ? (
          <Editor key={sel} slug={sel} server={server} onChanged={load} openStorePage={() => openStorePage(sel)} />
        ) : (
          games && (
            <div className="empty">
              <h1>Make something</h1>
              <p className="dim">Create a game here, then push builds with the condensate command.</p>
            </div>
          )
        )}
      </div>
    </div>
  );
}

function NewGame({ onCancel, onCreated }: { onCancel: () => void; onCreated: (slug: string) => void }) {
  const toast = useToast();
  const [title, setTitle] = useState("");
  const [slug, setSlug] = useState("");
  const [touched, setTouched] = useState(false);
  const autoSlug = title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40);
  const finalSlug = touched ? slug : autoSlug;
  return (
    <form
      className="new-game-form"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await api.createGame(finalSlug, title.trim());
          onCreated(finalSlug);
        } catch (err) {
          toast("error", errorMessage(err));
        }
      }}
    >
      <label className="field">
        Title
        <input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} required />
      </label>
      <label className="field">
        Game id
        <input
          value={finalSlug}
          onChange={(e) => {
            setTouched(true);
            setSlug(e.target.value);
          }}
          required
        />
      </label>
      <div className="row">
        <button className="plain-btn" disabled={!title.trim() || !finalSlug}>Create</button>
        <button type="button" className="text-btn" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}

function Editor({ slug, server, onChanged, openStorePage }: { slug: string; server: string; onChanged: () => void; openStorePage: () => void }) {
  const toast = useToast();
  const [game, setGame] = useState<GameDetail | null>(null);
  const [form, setForm] = useState({ title: "", summary: "", description: "", visibility: "public" });
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    api
      .game(slug)
      .then((g) => {
        setGame(g);
        setForm({ title: g.title, summary: g.summary, description: g.description, visibility: g.visibility });
      })
      .catch((e) => toast("error", errorMessage(e)));
  }, [slug, toast]);

  async function update(changes: GameChanges, note?: string) {
    try {
      setGame(await api.updateGame(slug, changes));
      onChanged();
      if (note) toast("info", note);
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  async function uploadArt(kind: "cover" | "banner") {
    try {
      const id = await pickAndUploadImage();
      if (id) await update(kind === "cover" ? { cover_image: id } : { banner_image: id }, "Artwork updated");
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  if (!game) return <div className="center-fill"><Spinner /></div>;
  const dirty =
    form.title !== game.title || form.summary !== game.summary || form.description !== game.description || form.visibility !== game.visibility;

  return (
    <div className="scroll page-pad">
      <div className="studio-head">
        <h1 className="page-title">{game.title}</h1>
        <button className="text-btn" onClick={openStorePage}>
          View store page <Icon name="chevron" size={14} />
        </button>
      </div>

      <section className="card">
        <h2>Artwork</h2>
        <div className="art-row">
          <ArtSlot label="Cover · 2:3, e.g. 600×900" ratio="2 / 3" game={{ slug, cover: game.cover, banner: null }} kind="cover" has={!!game.cover}
            onUpload={() => uploadArt("cover")} onRemove={() => update({ cover_image: "" })} />
          <ArtSlot label="Banner · 16:9, e.g. 1920×1080" ratio="16 / 9" game={{ slug, cover: null, banner: game.banner }} kind="banner" has={!!game.banner}
            onUpload={() => uploadArt("banner")} onRemove={() => update({ banner_image: "" })} />
        </div>
      </section>

      <section className="card">
        <h2>Store page</h2>
        <label className="field">
          Title
          <input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} maxLength={80} />
        </label>
        <label className="field">
          Summary (one line)
          <input value={form.summary} onChange={(e) => setForm({ ...form, summary: e.target.value })} maxLength={200} />
        </label>
        <label className="field">
          Description
          <textarea rows={6} value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} />
        </label>
        <label className="field">
          Visibility
          <select value={form.visibility} onChange={(e) => setForm({ ...form, visibility: e.target.value })}>
            <option value="public">Public: in the store for everyone</option>
            <option value="unlisted">Unlisted: only people you send it to</option>
            <option value="private">Private: only you</option>
          </select>
        </label>
        <div className="row">
          <button
            className="plain-btn"
            disabled={!dirty || saving}
            onClick={async () => {
              setSaving(true);
              await update(form, "Store page saved");
              setSaving(false);
            }}
          >
            Save
          </button>
        </div>
      </section>

      <section className="card">
        <h2>Versions</h2>
        {game.versions.length === 0 && <p className="dim">Nothing published yet. See "Publish with an AI agent" below, or run condensate push yourself.</p>}
        <div className="version-list">
          {game.versions.map((v) => (
            <div key={v.version} className="version-row">
              <b>v{v.version}</b>
              <span className="dim small">
                {timeAgo(v.created_at)} · {v.platforms.map(platformLabel).join(", ")}
              </span>
              <span className="version-log">{v.changelog}</span>
              <button
                className="mini-btn"
                title="Delete version"
                onClick={async () => {
                  if (!confirm(`Delete v${v.version}? Players will be offered the newest remaining version.`)) return;
                  try {
                    await api.deleteVersion(slug, v.version);
                    setGame({ ...game, versions: game.versions.filter((x) => x.version !== v.version) });
                    onChanged();
                  } catch (e) {
                    toast("error", errorMessage(e));
                  }
                }}
              >
                <Icon name="trash" size={15} />
              </button>
            </div>
          ))}
        </div>
      </section>

      <Tokens slug={slug} />

      <AgentInstructions slug={slug} title={game.title} server={server} />
    </div>
  );
}

/** Instructions to paste into an AI coding agent so it can publish this game with the CLI. */
function AgentInstructions({ slug, title, server }: { slug: string; title: string; server: string }) {
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const text = agentInstructions(slug, title, server);
  return (
    <section className="card">
      <h2>Publish with an AI agent</h2>
      <p className="dim">
        Paste these instructions into Claude, Codex or another coding agent working in your game's repo. It will set up{" "}
        <code>condensate.toml</code> and publish new versions with the <code>condensate</code> command, using this app's login.
      </p>
      <div className="agent-actions">
        <button
          className="plain-btn"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(text);
              toast("info", "Instructions copied. Paste them to your agent.");
            } catch {
              setOpen(true);
              toast("error", "Couldn't copy. Select the text below and copy it instead.");
            }
          }}
        >
          Copy instructions
        </button>
        <button className="text-btn" onClick={() => setOpen((o) => !o)}>
          {open ? "Hide" : "Show"} instructions
        </button>
      </div>
      {open && <pre className="snippet agent-text">{text}</pre>}
    </section>
  );
}

function agentInstructions(slug: string, title: string, server: string) {
  return `# Publishing "${title}" to Condensate

Condensate is our self-hosted game launcher. New builds of this game are published with the \`condensate\` command-line tool, which is already installed and logged in on this computer.

Game id: ${slug}
Server: ${server}

## One-time setup: condensate.toml
Create \`condensate.toml\` in the root of the game's repo. Keep only the platforms the game actually builds, and point \`dir\` and \`exe\` at the real export:

\`\`\`toml
game = "${slug}"
cover = "art/cover.png"      # optional store poster, portrait 2:3 (e.g. 600x900)
banner = "art/banner.png"    # optional store banner, landscape 16:9 (e.g. 1920x1080)

[[build]]
platform = "windows-x64"     # "windows-x64" or "linux-x64"
dir = "build/windows"        # folder holding the exported build, relative to this file
exe = "MyGame.exe"           # the executable, relative to dir, with / separators
# args = ["--fullscreen"]    # optional launch arguments
\`\`\`

## Publishing a release
1. Build or export the game into the \`dir\` folder(s).
2. Run: condensate push -m "<changelog>"

The changelog is required: one or two plain sentences a player would understand, for example "Added the boss fight. Fixed falling through the floor."
The version number goes up automatically (0.4.2 becomes 0.4.3). Only pass --version 1.0.0 when I ask for a specific version.
Only changed data is uploaded, so publishing again after a small fix is quick.

## Other commands
- condensate versions ${slug}  (published versions and their changelogs)
- condensate game info ${slug}
- condensate push --only windows-x64 -m "..."  (publish one platform from the project file)
- condensate game edit ${slug} --cover art/cover.png --banner art/banner.png  (update store art without a new build)
- condensate game delete-version ${slug} <version>  (remove a broken release; players get the newest remaining one)
- Put --json after \`condensate\` for machine-readable output, e.g. condensate --json push -m "..."

## If something fails
- "not logged in": ask me to log in to the Condensate app, then run the command again.
- "launch executable ... is not part of the build": exe must be a path inside dir.
- "version X already exists": leave out --version.
- File names can't contain : * ? " < > | or end with a dot or space.
- Running somewhere other than my computer (CI, a server)? Set CONDENSATE_SERVER=${server} and CONDENSATE_TOKEN to a publish token from the Studio page.
`;
}

function ArtSlot(props: {
  label: string;
  ratio: string;
  game: { slug: string; cover: string | null; banner: string | null };
  kind: "cover" | "banner";
  has: boolean;
  onUpload: () => void;
  onRemove: () => void;
}) {
  return (
    <div className="art-slot">
      <div className="art-frame" style={{ aspectRatio: props.ratio }} onClick={props.onUpload}>
        <GameImage game={props.game} kind={props.kind} />
        <div className="art-hover">{props.has ? "Replace" : "Upload"}</div>
      </div>
      <div className="art-meta">
        <span className="dim small">{props.label}</span>
        {props.has && (
          <button className="text-btn danger small" onClick={props.onRemove}>
            Remove
          </button>
        )}
      </div>
    </div>
  );
}

/** Publish tokens let a build agent push this game without your password. */
function Tokens({ slug }: { slug: string }) {
  const toast = useToast();
  const [tokens, setTokens] = useState<TokenInfo[]>([]);
  const [fresh, setFresh] = useState<string | null>(null);

  const load = useCallback(() => {
    api.listTokens().then((t) => setTokens(t.filter((x) => x.kind === "publish" && x.game === slug))).catch(() => {});
  }, [slug]);
  useEffect(load, [load]);

  return (
    <section className="card">
      <h2>Publish tokens</h2>
      <p className="dim">For build agents and AI assistants: set <code>CONDENSATE_TOKEN</code> and they can push this game, nothing else.</p>
      {fresh && (
        <div className="token-fresh">
          <code>{fresh}</code>
          <button className="text-btn" onClick={() => navigator.clipboard.writeText(fresh)}>Copy</button>
          <span className="dim small">Shown once. Copy it now.</span>
        </div>
      )}
      <div className="version-list">
        {tokens.map((t) => (
          <div key={t.id} className="version-row">
            <b>{t.name}</b>
            <span className="dim small">created {timeAgo(t.created_at)} · last used {timeAgo(t.last_used_at)}</span>
            <span />
            <button
              className="mini-btn"
              title="Revoke"
              onClick={async () => {
                try {
                  await api.revokeToken(t.id);
                  load();
                } catch (e) {
                  toast("error", errorMessage(e));
                }
              }}
            >
              <Icon name="trash" size={15} />
            </button>
          </div>
        ))}
      </div>
      <div className="row">
        <button
          className="plain-btn outline"
          onClick={async () => {
            const name = prompt("Name this token (e.g. 'Claude on my laptop')");
            if (!name) return;
            try {
              const created = await api.createToken(name, slug);
              setFresh(created.token);
              navigator.clipboard.writeText(created.token).catch(() => {});
              load();
            } catch (e) {
              toast("error", errorMessage(e));
            }
          }}
        >
          <Icon name="plus" size={15} /> New token
        </button>
      </div>
    </section>
  );
}
