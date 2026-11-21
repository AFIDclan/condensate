import { createContext, ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, ChatMessage, Conversation, errorMessage, FriendsResponse, WsEvent } from "./api";
import { useToast } from "./components";

interface Social {
  friends: FriendsResponse | null;
  conversations: Conversation[];
  /** Unread messages across all conversations. */
  unread: number;
  connected: boolean;
  refreshFriends: () => Promise<void>;
  /** The chat currently on screen (its messages don't count as unread). */
  setOpenChat: (username: string | null) => void;
  /** Subscribe to new messages for one conversation. */
  onMessage: (f: (m: ChatMessage) => void) => () => void;
  markRead: (username: string) => void;
}

const SocialContext = createContext<Social | null>(null);

export function useSocial(): Social {
  const s = useContext(SocialContext);
  if (!s) throw new Error("useSocial outside SocialProvider");
  return s;
}

export function SocialProvider({ me, children }: { me: string; children: ReactNode }) {
  const toast = useToast();
  const [friends, setFriends] = useState<FriendsResponse | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [connected, setConnected] = useState(false);
  const openChat = useRef<string | null>(null);
  const listeners = useRef(new Set<(m: ChatMessage) => void>());

  const refreshFriends = useCallback(async () => {
    try {
      setFriends(await api.friends());
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }, [toast]);

  const refreshConversations = useCallback(async () => {
    try {
      setConversations(await api.conversations());
    } catch {
      /* shown elsewhere */
    }
  }, []);

  const markRead = useCallback((username: string) => {
    setConversations((cs) => cs.map((c) => (c.user.username === username ? { ...c, unread: 0 } : c)));
    api.markRead(username).catch(() => {});
  }, []);

  useEffect(() => {
    refreshFriends();
    refreshConversations();
    const unlisten = [
      listen<boolean>("realtime", (e) => {
        setConnected(e.payload);
        // Catch up on anything missed while disconnected.
        if (e.payload) {
          refreshFriends();
          refreshConversations();
        }
      }),
      listen<WsEvent>("ws-event", (e) => {
        const ev = e.payload;
        if (ev.type === "presence") {
          setFriends((f) => f && { ...f, friends: f.friends.map((x) => (x.user.username === ev.user ? { ...x, presence: ev.presence } : x)) });
        } else if (ev.type === "friends_changed") {
          api.friends().then((f) => {
            setFriends((old) => {
              const before = new Set(old?.incoming.map((u) => u.username));
              const fresh = f.incoming.filter((u) => !before.has(u.username));
              if (old && fresh.length) toast("info", `${fresh[0].display_name} sent you a friend request`);
              return f;
            });
          });
        } else if (ev.type === "message") {
          const m = ev.message;
          const other = m.from === me ? m.to : m.from;
          const incoming = m.from !== me;
          const viewing = openChat.current === other;
          listeners.current.forEach((f) => f(m));
          setConversations((cs) => {
            const existing = cs.find((c) => c.user.username === other);
            if (!existing) {
              refreshConversations();
              return cs;
            }
            const updated = { ...existing, last: m, unread: existing.unread + (incoming && !viewing ? 1 : 0) };
            return [updated, ...cs.filter((c) => c !== existing)];
          });
          if (incoming && viewing) api.markRead(other).catch(() => {});
          if (incoming && !viewing) toast("info", `${other}: ${m.body.length > 80 ? m.body.slice(0, 80) + "…" : m.body}`);
        }
      }),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  }, [me, refreshFriends, refreshConversations, toast]);

  const value = useMemo<Social>(
    () => ({
      friends,
      conversations,
      unread: conversations.reduce((n, c) => n + c.unread, 0),
      connected,
      refreshFriends,
      setOpenChat: (u) => {
        openChat.current = u;
      },
      onMessage: (f) => {
        listeners.current.add(f);
        return () => listeners.current.delete(f);
      },
      markRead,
    }),
    [friends, conversations, connected, refreshFriends, markRead],
  );

  return <SocialContext.Provider value={value}>{children}</SocialContext.Provider>;
}
