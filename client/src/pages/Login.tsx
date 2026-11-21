import { FormEvent, useState } from "react";
import { api, errorMessage } from "../api";
import { Wordmark } from "../App";

const DEFAULT_SERVER = "https://oort-cloud.tail4b4a9d.ts.net";

export default function Login({ server: initialServer, onDone }: { server: string; onDone: () => void }) {
  const [mode, setMode] = useState<"login" | "register">("login");
  const [server, setServer] = useState(initialServer || DEFAULT_SERVER);
  const [showServer, setShowServer] = useState(false);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === "login") await api.login(server, username, password);
      else await api.register(server, username, password, invite);
      onDone();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login">
      <form className="login-card" onSubmit={submit}>
        <Wordmark />
        <div className="segmented">
          <button type="button" className={mode === "login" ? "on" : ""} onClick={() => setMode("login")}>
            Log in
          </button>
          <button type="button" className={mode === "register" ? "on" : ""} onClick={() => setMode("register")}>
            Create account
          </button>
        </div>
        <label className="field">
          Username
          <input value={username} onChange={(e) => setUsername(e.target.value)} autoFocus autoComplete="username" required />
        </label>
        <label className="field">
          Password
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete={mode === "login" ? "current-password" : "new-password"}
            minLength={mode === "register" ? 8 : undefined}
            required
          />
        </label>
        {mode === "register" && (
          <label className="field">
            Invite code
            <input value={invite} onChange={(e) => setInvite(e.target.value)} placeholder="XXXX-XXXX-XXXX" required />
          </label>
        )}
        {showServer && (
          <label className="field">
            Server
            <input value={server} onChange={(e) => setServer(e.target.value)} required />
          </label>
        )}
        {error && <div className="error">{error}</div>}
        <button className="btn" disabled={busy}>
          <span>{busy ? "…" : mode === "login" ? "Log in" : "Create account"}</span>
        </button>
        {!showServer && (
          <button type="button" className="server-link" onClick={() => setShowServer(true)}>
            Server: {server.replace(/^https?:\/\//, "")}
          </button>
        )}
      </form>
    </div>
  );
}
