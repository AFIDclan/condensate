import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, LOGGED_OUT_EVENT, Session } from "./api";
import { Spinner, ToastProvider } from "./components";
import Login from "./pages/Login";
import Library from "./pages/Library";
import Store from "./pages/Store";
import GamePage from "./pages/GamePage";
import Account from "./pages/Account";

export type Route =
  | { page: "library"; slug?: string }
  | { page: "store" }
  | { page: "game"; slug: string }
  | { page: "account" };

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [offline, setOffline] = useState<string | null>(null);
  const [forceLogin, setForceLogin] = useState(false);

  const load = useCallback(async () => {
    try {
      setSession(await api.session());
      setOffline(null);
    } catch (e) {
      setOffline(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    load();
    const onLoggedOut = () => load();
    window.addEventListener(LOGGED_OUT_EVENT, onLoggedOut);
    return () => window.removeEventListener(LOGGED_OUT_EVENT, onLoggedOut);
  }, [load]);

  if (offline && !forceLogin) {
    return (
      <div className="center-screen">
        <div className="card narrow">
          <Logo />
          <p className="muted">{offline}</p>
          <div className="row">
            <button className="primary" onClick={load}>Retry</button>
            <button onClick={() => setForceLogin(true)}>Change server</button>
          </div>
        </div>
      </div>
    );
  }
  if (!session && !forceLogin) {
    return (
      <div className="center-screen">
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
      <Shell session={session} onLogout={async () => { await api.logout(); load(); }} />
    </ToastProvider>
  );
}

export function Logo() {
  return (
    <div className="logo">
      <img src="/icon.svg" alt="" />
      <span>Condensate</span>
    </div>
  );
}

function Shell({ session, onLogout }: { session: Session; onLogout: () => void }) {
  const [route, setRoute] = useState<Route>({ page: "library" });
  const user = session.user!;

  const tab = (page: Route["page"], label: string) => (
    <button
      className={`tab ${route.page === page || (page === "store" && route.page === "game") ? "active" : ""}`}
      onClick={() => setRoute({ page } as Route)}
    >
      {label}
    </button>
  );

  return (
    <div className="shell">
      <header className="topbar">
        <Logo />
        <nav>
          {tab("library", "Library")}
          {tab("store", "Store")}
          <button className="tab" disabled title="Coming soon">Friends</button>
        </nav>
        <button className={`user-chip ${route.page === "account" ? "active" : ""}`} onClick={() => setRoute({ page: "account" })}>
          <span className="avatar">{user.display_name[0]?.toUpperCase()}</span>
          {user.display_name}
        </button>
      </header>
      <main className="content">
        {route.page === "library" && <Library selected={route.slug} navigate={setRoute} />}
        {route.page === "store" && <Store navigate={setRoute} />}
        {route.page === "game" && <GamePage slug={route.slug} navigate={setRoute} />}
        {route.page === "account" && <Account session={session} onLogout={onLogout} />}
      </main>
    </div>
  );
}
