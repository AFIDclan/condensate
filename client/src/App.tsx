import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "@fontsource/bebas-neue";
import "@fontsource-variable/inter";
import "./theme.css";
import "./theme-social.css";
import { api, errorMessage, LOGGED_OUT_EVENT, Session, setServer, User } from "./api";
import { Avatar, Spinner, ToastProvider } from "./components";
import { SocialProvider, useSocial } from "./social";
import { installUpdate, useAvailableUpdate } from "./updater";
import { useActions, useLibrary } from "./data";
import Login from "./pages/Login";
import Library from "./pages/Library";
import Store from "./pages/Store";
import Game from "./pages/Game";
import Account from "./pages/Account";
import Friends from "./pages/Friends";
import Studio from "./pages/Studio";
import { VoiceDock } from "./voice";

export type Route =
  | { page: "library" }
  | { page: "store" }
  | { page: "game"; slug: string; from: "library" | "store" | "studio" }
  | { page: "friends" }
  | { page: "studio" }
  | { page: "account" };

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [offline, setOffline] = useState<string | null>(null);
  const [forceLogin, setForceLogin] = useState(false);

  const load = useCallback(async () => {
    try {
      const s = await api.session();
      setServer(s.server);
      setSession(s);
      setOffline(null);
    } catch (e) {
      setOffline(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    load();
    const onLoggedOut = () => load();
    window.addEventListener(LOGGED_OUT_EVENT, onLoggedOut);
    // The CLI shares our login and may relaunch us to ask for one.
    const unlisten = listen("refresh-session", () => load());
    return () => {
      window.removeEventListener(LOGGED_OUT_EVENT, onLoggedOut);
      unlisten.then((f) => f());
    };
  }, [load]);

  if (offline && !forceLogin) {
    return (
      <div className="login">
        <div className="login-card">
          <Wordmark />
          <p className="dim">{offline}</p>
          <div style={{ display: "flex", gap: 10 }}>
            <button className="plain-btn" onClick={load}>Retry</button>
            <button className="plain-btn outline" onClick={() => setForceLogin(true)}>Change server</button>
          </div>
        </div>
      </div>
    );
  }
  if (!session && !forceLogin) {
    return (
      <div className="center-fill">
        <Spinner />
      </div>
    );
  }
  if (forceLogin || !session?.user) {
    return (
      <Login
        server={session?.server ?? ""}
        onDone={() => {
          setForceLogin(false);
          load();
        }}
      />
    );
  }
  return (
    <ToastProvider>
      <SocialProvider me={session.user.username}>
        <Shell
          session={session}
          onLogout={async () => {
            await api.logout();
            load();
          }}
          onUserChanged={(user) => setSession({ ...session, user })}
        />
      </SocialProvider>
    </ToastProvider>
  );
}

export function Wordmark() {
  return (
    <div className="wordmark">
      <img src="/icon.svg" alt="" />
      Condensate
    </div>
  );
}

function Shell({ session, onLogout, onUserChanged }: { session: Session; onLogout: () => void; onUserChanged: (u: User) => void }) {
  const [route, setRoute] = useState<Route>({ page: "library" });
  const { items, refresh } = useLibrary();
  const actions = useActions(refresh);
  const social = useSocial();
  const update = useAvailableUpdate();
  const user = session.user!;
  const section = route.page === "game" ? route.from : route.page;
  const friendBadge = social.unread + (social.friends?.incoming.length ?? 0);

  const tab = (page: "library" | "store" | "friends" | "studio", label: string, badge = 0) => (
    <button className={section === page ? "on" : ""} onClick={() => setRoute({ page })}>
      {label}
      {badge > 0 && <span className="tab-badge">{badge}</span>}
    </button>
  );

  return (
    <div className="shell">
      {update && (
        <div className="update-bar">
          Condensate {update} is ready.
          <button className="plain-btn" onClick={() => installUpdate()}>
            Restart to update
          </button>
        </div>
      )}
      <header className="topbar">
        <Wordmark />
        <nav className="tabs">
          {tab("library", "Library")}
          {tab("store", "Store")}
          {tab("friends", "Friends", friendBadge)}
          {tab("studio", "Studio")}
        </nav>
        <div className="grow" />
        <button className={`me ${section === "account" ? "on" : ""}`} onClick={() => setRoute({ page: "account" })} title="Account & settings">
          <Avatar user={user} size={34} status={social.connected ? "online" : "offline"} />
          <span>{user.display_name}</span>
        </button>
      </header>
      <main className="content">
        {route.page === "library" && (
          <Library items={items} actions={actions} open={(slug) => setRoute({ page: "game", slug, from: "library" })} goStore={() => setRoute({ page: "store" })} />
        )}
        {route.page === "store" && <Store items={items} open={(slug) => setRoute({ page: "game", slug, from: "store" })} />}
        {route.page === "game" && (
          <Game
            key={route.slug}
            slug={route.slug}
            item={items?.find((i) => i.game.slug === route.slug)}
            actions={actions}
            backLabel={route.from === "store" ? "Store" : route.from === "studio" ? "Studio" : "Library"}
            onBack={() => setRoute({ page: route.from })}
          />
        )}
        {route.page === "friends" && <Friends me={user.username} />}
        {route.page === "studio" && <Studio server={session.server} openStorePage={(slug) => setRoute({ page: "game", slug, from: "studio" })} />}
        {route.page === "account" && <Account session={session} onLogout={onLogout} onUserChanged={onUserChanged} />}
      </main>
      <VoiceDock />
    </div>
  );
}
