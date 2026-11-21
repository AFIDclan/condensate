import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { disable as disableAutostart, enable as enableAutostart, isEnabled as autostartEnabled } from "@tauri-apps/plugin-autostart";
import { api, errorMessage, Invite, Session, timeAgo, User } from "../api";
import { Avatar, pickAndUploadImage, useToast } from "../components";
import { checkForUpdate } from "../updater";

export default function Account({ session, onLogout, onUserChanged }: { session: Session; onLogout: () => void; onUserChanged: (u: User) => void }) {
  const toast = useToast();
  const [user, setUser] = useState(session.user!);
  const [displayName, setDisplayName] = useState(user.display_name);
  const [bio, setBio] = useState(user.bio);
  const [invites, setInvites] = useState<Invite[]>([]);
  const [libraryDir, setLibraryDir] = useState(session.library_dir);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [version, setVersion] = useState("");

  useEffect(() => {
    api.listInvites().then(setInvites).catch((e) => toast("error", errorMessage(e)));
    autostartEnabled().then(setAutostart).catch(() => setAutostart(null));
    getVersion().then(setVersion).catch(() => {});
  }, [toast]);

  const changed = (u: User) => {
    setUser(u);
    onUserChanged(u);
  };

  async function saveProfile() {
    try {
      changed(await api.updateProfile(displayName, bio));
      toast("info", "Profile saved");
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  async function changeAvatar() {
    try {
      const id = await pickAndUploadImage();
      if (id) changed(await api.updateProfile(null, null, id));
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

  async function moveLibrary() {
    const picked = await open({ directory: true, defaultPath: libraryDir, title: "Choose where games are installed" });
    if (typeof picked !== "string") return;
    try {
      setLibraryDir(await api.setLibraryDir(picked));
      toast("info", "Games folder changed");
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }

  return (
    <div className="scroll page-pad">
      <div className="account">
        <div className="profile-head">
          <button className="avatar-edit" onClick={changeAvatar} title="Change picture">
            <Avatar user={user} size={96} />
            <span>Change</span>
          </button>
          <div>
            <h1 className="page-title">{user.display_name}</h1>
            <div className="dim">
              @{user.username}
              {user.is_admin && " · admin"} · joined {timeAgo(user.created_at)}
            </div>
          </div>
        </div>

        <section className="card">
          <h2>Profile</h2>
          <label className="field">
            Display name
            <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} maxLength={40} />
          </label>
          <label className="field">
            Bio
            <textarea value={bio} onChange={(e) => setBio(e.target.value)} rows={3} maxLength={1000} />
          </label>
          <div className="row">
            <button className="plain-btn" onClick={saveProfile}>Save</button>
            {user.avatar && (
              <button className="text-btn danger" onClick={() => api.updateProfile(null, null, "").then(changed).catch((e) => toast("error", errorMessage(e)))}>
                Remove picture
              </button>
            )}
          </div>
        </section>

        <section className="card">
          <h2>Invite friends</h2>
          <p className="dim">Each code works once. Send it with the installer link: {session.server}/download/</p>
          <div className="row">
            <button className="plain-btn" onClick={newInvite}>Create invite code</button>
          </div>
          <ul className="invites">
            {invites.map((i) => (
              <li key={i.code}>
                <code>{i.code}</code>
                <span className="dim small">{i.used_by ? `used by ${i.used_by}` : `unused · ${timeAgo(i.created_at)}`}</span>
                {!i.used_by && (
                  <button className="link small" onClick={() => navigator.clipboard.writeText(i.code)}>Copy</button>
                )}
              </li>
            ))}
          </ul>
        </section>

        <section className="card">
          <h2>Settings</h2>
          <div className="setting">
            <div>
              <b>Games folder</b>
              <div className="dim small selectable">{libraryDir}</div>
            </div>
            <button className="plain-btn outline" onClick={moveLibrary}>Change…</button>
          </div>
          <div className="setting">
            <div>
              <b>Start with Windows</b>
              <div className="dim small">Open Condensate minimized when you sign in, so friends see you online.</div>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={!!autostart}
                disabled={autostart === null}
                onChange={async (e) => {
                  try {
                    if (e.target.checked) await enableAutostart();
                    else await disableAutostart();
                    setAutostart(e.target.checked);
                  } catch (err) {
                    toast("error", errorMessage(err));
                  }
                }}
              />
              <span />
            </label>
          </div>
          <div className="setting">
            <div>
              <b>Condensate {version}</b>
              <div className="dim small">Updates install automatically when you restart.</div>
            </div>
            <button
              className="plain-btn outline"
              onClick={async () => {
                try {
                  const found = await checkForUpdate(true);
                  toast("info", found ? `Version ${found} is ready: restart to install` : "You're up to date");
                } catch (e) {
                  toast("error", `Couldn't check for updates: ${errorMessage(e)}`);
                }
              }}
            >
              Check for updates
            </button>
          </div>
        </section>

        <section className="card">
          <h2>This computer</h2>
          <div className="kv"><span>Server</span><code>{session.server}</code></div>
          <div className="kv"><span>Platform</span><code>{session.platform ?? "unsupported"}</code></div>
          <div className="row">
            <button className="plain-btn danger" onClick={onLogout}>Log out</button>
          </div>
        </section>
      </div>
    </div>
  );
}
