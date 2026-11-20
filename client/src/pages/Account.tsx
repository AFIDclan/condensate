import { useEffect, useState } from "react";
import { api, errorMessage, Invite, Session, timeAgo } from "../api";
import { useToast } from "../components";

export default function Account({ session, onLogout }: { session: Session; onLogout: () => void }) {
  const toast = useToast();
  const user = session.user!;
  const [displayName, setDisplayName] = useState(user.display_name);
  const [bio, setBio] = useState(user.bio);
  const [invites, setInvites] = useState<Invite[]>([]);

  useEffect(() => {
    api.listInvites().then(setInvites).catch((e) => toast("error", errorMessage(e)));
  }, [toast]);

  async function saveProfile() {
    try {
      await api.updateProfile(displayName, bio);
      toast("info", "Profile saved");
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  async function newInvite() {
    try {
      const invite = await api.createInvite();
      setInvites((list) => [invite, ...list]);
      await navigator.clipboard.writeText(invite.code).catch(() => {});
      toast("info", `Invite ${invite.code} copied to clipboard`);
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  return (
    <div className="page account">
      <h1>Account</h1>

      <section className="block card">
        <h3>Profile</h3>
        <div className="muted small">
          @{user.username}
          {user.is_admin && " · admin"} · joined {timeAgo(user.created_at)}
        </div>
        <label>
          Display name
          <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} maxLength={40} />
        </label>
        <label>
          Bio
          <textarea value={bio} onChange={(e) => setBio(e.target.value)} rows={3} maxLength={1000} />
        </label>
        <div className="row">
          <button className="primary" onClick={saveProfile}>Save</button>
        </div>
      </section>

      <section className="block card">
        <h3>Invite friends</h3>
        <p className="muted">Each code works once. Send it with the Condensate installer.</p>
        <div className="row">
          <button className="primary" onClick={newInvite}>Create invite code</button>
        </div>
        <ul className="invites">
          {invites.map((i) => (
            <li key={i.code}>
              <code>{i.code}</code>
              <span className="muted small">{i.used_by ? `used by ${i.used_by}` : `unused · ${timeAgo(i.created_at)}`}</span>
              {!i.used_by && (
                <button className="link" onClick={() => navigator.clipboard.writeText(i.code)}>Copy</button>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="block card">
        <h3>Publishing games</h3>
        <p className="muted">
          Use the <code>condensate</code> command-line tool: <code>condensate game create</code>, then{" "}
          <code>condensate push -m "what changed"</code>. Give build agents a publish token from{" "}
          <code>condensate token create</code>.
        </p>
      </section>

      <section className="block card">
        <h3>This computer</h3>
        <div className="kv"><span>Server</span><code>{session.server}</code></div>
        <div className="kv"><span>Games folder</span><code>{session.library_dir}</code></div>
        <div className="kv"><span>Platform</span><code>{session.platform ?? "unsupported"}</code></div>
        <div className="row">
          <button className="danger-outline" onClick={onLogout}>Log out</button>
        </div>
      </section>
    </div>
  );
}
