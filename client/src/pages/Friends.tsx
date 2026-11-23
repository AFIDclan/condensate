import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, ChatMessage, errorMessage, Friend, timeAgo, User } from "../api";
import { Avatar, Spinner, useToast } from "../components";
import { Icon } from "../icons";
import { useContextMenu } from "../menu";
import { useSocial } from "../social";

export function presenceText(f: Friend): string {
  if (f.presence.status === "in_game") return `Playing ${f.presence.game?.title ?? "a game"}`;
  if (f.presence.status === "online") return "Online";
  return f.presence.last_seen_at ? `Last seen ${timeAgo(f.presence.last_seen_at)}` : "Offline";
}

export default function Friends({ me, initialChat }: { me: string; initialChat?: string }) {
  const social = useSocial();
  const toast = useToast();
  const menu = useContextMenu();
  const [chat, setChat] = useState<string | null>(initialChat ?? null);
  const data = social.friends;

  const act = async (f: () => Promise<void>) => {
    try {
      await f();
      await social.refreshFriends();
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };

  const unreadFor = (username: string) => social.conversations.find((c) => c.user.username === username)?.unread ?? 0;
  const online = data?.friends.filter((f) => f.presence.status !== "offline").length ?? 0;
  const current = data?.friends.find((f) => f.user.username === chat);

  return (
    <div className="friends">
      <aside className="friends-side">
        <AddFriend onAdded={() => social.refreshFriends()} />
        {!data && <Spinner />}
        {data && data.incoming.length > 0 && (
          <section>
            <h3 className="side-title">Requests · {data.incoming.length}</h3>
            {data.incoming.map((u) => (
              <div key={u.username} className="person">
                <Avatar user={u} size={38} />
                <div className="person-text">
                  <b>{u.display_name}</b>
                  <span>@{u.username} wants to be friends</span>
                </div>
                <button className="mini-btn accept" title="Accept" onClick={() => act(() => api.addFriend(u.username))}>
                  <Icon name="check" size={16} />
                </button>
                <button className="mini-btn" title="Decline" onClick={() => act(() => api.removeFriend(u.username))}>
                  <Icon name="x" size={16} />
                </button>
              </div>
            ))}
          </section>
        )}
        {data && (
          <section className="friends-list">
            <h3 className="side-title">
              Friends · {online} online
            </h3>
            {data.friends.length === 0 && <p className="dim small">No friends yet. Search for someone above.</p>}
            {data.friends.map((f) => {
              const unread = unreadFor(f.user.username);
              return (
                <button
                  key={f.user.username}
                  className={`person clickable ${chat === f.user.username ? "on" : ""} ${f.presence.status === "offline" ? "offline" : ""}`}
                  onClick={() => setChat(f.user.username)}
                  onContextMenu={(e) =>
                    menu.open(e, [
                      ...(f.presence.lobby ? [{ label: `Join in ${f.presence.lobby.game.title}`, icon: "play" as const, onClick: () => social.joinLobby(f.presence.lobby!) }] : []),
                      { label: "Message", icon: "notes", onClick: () => setChat(f.user.username) },
                      {
                        label: "Remove friend",
                        icon: "x",
                        danger: true,
                        onClick: () => confirm(`Remove ${f.user.display_name} as a friend?`) && act(() => api.removeFriend(f.user.username)),
                      },
                    ])
                  }
                >
                  <Avatar user={f.user} size={38} status={f.presence.status} />
                  <div className="person-text">
                    <b>{f.user.display_name}</b>
                    <span className={f.presence.status === "in_game" ? "in-game" : ""}>{presenceText(f)}</span>
                  </div>
                  {unread > 0 && <span className="badge">{unread}</span>}
                  {f.presence.lobby && (
                    <span
                      className="join-chip"
                      role="button"
                      title={`Join ${f.user.display_name} in ${f.presence.lobby.game.title}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        social.joinLobby(f.presence.lobby!);
                      }}
                    >
                      Join{f.presence.lobby.recording ? " · REC" : ""}
                    </span>
                  )}
                </button>
              );
            })}
          </section>
        )}
        {data && data.outgoing.length > 0 && (
          <section>
            <h3 className="side-title">Sent requests</h3>
            {data.outgoing.map((u) => (
              <div key={u.username} className="person offline">
                <Avatar user={u} size={32} />
                <div className="person-text">
                  <b>{u.display_name}</b>
                  <span>Waiting for them to accept</span>
                </div>
                <button className="mini-btn" title="Cancel request" onClick={() => act(() => api.removeFriend(u.username))}>
                  <Icon name="x" size={16} />
                </button>
              </div>
            ))}
          </section>
        )}
        {!social.connected && <p className="dim small offline-note">Reconnecting to live updates…</p>}
      </aside>
      <section className="chat">
        {current ? (
          <Chat key={current.user.username} friend={current} me={me} />
        ) : (
          <div className="empty">
            <h1>Pick a friend</h1>
            <p className="dim">See what they're playing and send them a message.</p>
          </div>
        )}
      </section>
      {menu.element}
    </div>
  );
}

function AddFriend({ onAdded }: { onAdded: () => void }) {
  const toast = useToast();
  const [q, setQ] = useState("");
  const [results, setResults] = useState<User[]>([]);
  const social = useSocial();
  const known = new Set([
    ...(social.friends?.friends.map((f) => f.user.username) ?? []),
    ...(social.friends?.outgoing.map((u) => u.username) ?? []),
  ]);

  useEffect(() => {
    if (!q.trim()) return setResults([]);
    const t = setTimeout(() => api.searchUsers(q.trim()).then(setResults).catch(() => {}), 200);
    return () => clearTimeout(t);
  }, [q]);

  return (
    <div className="add-friend">
      <label className="search">
        <Icon name="search" size={16} />
        <input placeholder="Add a friend by name" value={q} onChange={(e) => setQ(e.target.value)} />
      </label>
      {results.length > 0 && (
        <div className="add-results">
          {results.map((u) => (
            <div key={u.username} className="person">
              <Avatar user={u} size={32} />
              <div className="person-text">
                <b>{u.display_name}</b>
                <span>@{u.username}</span>
              </div>
              {known.has(u.username) ? (
                <span className="dim small">Added</span>
              ) : (
                <button
                  className="mini-btn accept"
                  title="Send friend request"
                  onClick={async () => {
                    try {
                      await api.addFriend(u.username);
                      toast("info", `Friend request sent to ${u.display_name}`);
                      setQ("");
                      onAdded();
                    } catch (e) {
                      toast("error", errorMessage(e));
                    }
                  }}
                >
                  <Icon name="plus" size={16} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Chat({ friend, me }: { friend: Friend; me: string }) {
  const social = useSocial();
  const toast = useToast();
  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const username = friend.user.username;

  useEffect(() => {
    social.setOpenChat(username);
    api.messages(username).then(setMessages).catch((e) => toast("error", errorMessage(e)));
    social.markRead(username);
    const off = social.onMessage((m) => {
      if (m.from === username || m.to === username) setMessages((list) => (list && !list.some((x) => x.id === m.id) ? [...list, m] : list));
    });
    return () => {
      off();
      social.setOpenChat(null);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [username]);

  useLayoutEffect(() => {
    scroller.current?.scrollTo({ top: scroller.current.scrollHeight });
  }, [messages]);

  async function send() {
    const body = draft.trim();
    if (!body || sending) return;
    setSending(true);
    try {
      const m = await api.sendMessage(username, body);
      setMessages((list) => (list && !list.some((x) => x.id === m.id) ? [...list, m] : list));
      setDraft("");
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setSending(false);
    }
  }

  return (
    <>
      <header className="chat-head">
        <Avatar user={friend.user} size={46} status={friend.presence.status} />
        <div>
          <div className="chat-name">{friend.user.display_name}</div>
          <div className={`dim small ${friend.presence.status === "in_game" ? "in-game" : ""}`}>
            @{friend.user.username} · {presenceText(friend)}
          </div>
        </div>
        {friend.presence.lobby && (
          <button className="plain-btn join-btn" onClick={() => social.joinLobby(friend.presence.lobby!)}>
            Join game · {friend.presence.lobby.members}/{friend.presence.lobby.max_members}
          </button>
        )}
      </header>
      <div className="chat-scroll" ref={scroller}>
        {!messages && <Spinner />}
        {messages?.length === 0 && <p className="dim chat-start">This is the start of your chat with {friend.user.display_name}.</p>}
        {messages?.map((m, i) => {
          const mine = m.from === me;
          const prev = messages[i - 1];
          const grouped = prev && prev.from === m.from && m.created_at - prev.created_at < 300;
          return (
            <div key={m.id} className={`bubble-row ${mine ? "mine" : ""} ${grouped ? "grouped" : ""}`}>
              <div className="bubble" title={new Date(m.created_at * 1000).toLocaleString()}>
                {m.body}
              </div>
            </div>
          );
        })}
      </div>
      <div className="composer">
        <textarea
          rows={1}
          placeholder={`Message ${friend.user.display_name}`}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <button className="plain-btn" disabled={!draft.trim() || sending} onClick={send}>
          Send
        </button>
      </div>
    </>
  );
}
