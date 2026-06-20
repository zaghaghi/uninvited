import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentActivity, GameView, Party } from "../types";
import { DUR, EASE, gsap, reduceMotion, useGSAP } from "../anim";

export function GameScreen({
  game,
  activities,
}: {
  game: GameView;
  activities: AgentActivity[];
}) {
  // The public board of every possible party. Constant for a build, so we
  // fetch it once when the game screen mounts.
  const [parties, setParties] = useState<Party[]>([]);
  useEffect(() => {
    invoke<Party[]>("all_parties").then(setParties).catch(() => {});
  }, []);

  const thoughts = activities.filter((a) => a.kind === "thinking");

  // Settle the table in on mount: the transcript fades up while the sidebar
  // panels deal in one after another, so the scene assembles rather than blinks.
  const root = useRef<HTMLDivElement | null>(null);
  useGSAP(
    () => {
      if (reduceMotion()) return;
      gsap.from(".tlog", { opacity: 0, y: 10, duration: DUR.slow, ease: EASE });
      gsap.from(".sidebar > *", {
        opacity: 0,
        y: 14,
        duration: DUR.base,
        ease: EASE,
        stagger: 0.08,
        delay: 0.1,
      });
    },
    { scope: root },
  );

  // The conversation is the main view; role, the party board, and (debug)
  // thoughts sit in a slim side column.
  return (
    <div className="game" ref={root}>
      <div className="game-main">
        <TranscriptLog game={game} />
        <aside className="sidebar">
          <RolePanel game={game} />
          <GuestList game={game} activeId={activeAsker(game)} />
          <LocationBoard parties={parties} game={game} />
          {thoughts.length > 0 && <ThoughtsPanel game={game} thoughts={thoughts} />}
        </aside>
      </div>
      <InputBar game={game} parties={parties} />
    </div>
  );
}

// The board of every possible party, shown to everyone all game. The outsider
// uses it to narrow down where they are; invited guests see their own party
// marked so the board doubles as a reminder. The occasion is the headline
// (bolder) with the venue beneath it. The active party is matched on the full
// pair — the venue alone is ambiguous (two parties share "Backyard").
function LocationBoard({ parties, game }: { parties: Party[]; game: GameView }) {
  if (parties.length === 0) return null;
  return (
    <div className="locboard">
      <div className="locboard-title">Possible parties</div>
      <ul className="locboard-list">
        {parties.map((p) => {
          const active =
            p.location === game.location && p.occasion === game.occasion;
          return (
            <li key={`${p.location}|${p.occasion}`} className={active ? "is-active" : ""}>
              <span className="party-occ">{p.occasion}</span>
              <span className="party-loc">{p.location}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function RolePanel({ game }: { game: GameView }) {
  return (
    <div className="rolecard">
      {game.mode === "bartender" ? (
        <>
          <div className="role-tag invited">You're the bartender</div>
          <div className="role-line">
            <strong className="party-occ">{game.occasion}</strong>
            {game.location && <span className="party-loc"> — {game.location}</span>}
          </div>
          <p className="hint">Listen in at the bar, then call out who wasn't really invited.</p>
        </>
      ) : game.humanInvited ? (
        <>
          <div className="role-tag invited">You're invited</div>
          <div className="role-line">
            <strong className="party-occ">{game.occasion}</strong>
            {game.location && <span className="party-loc"> — {game.location}</span>}
          </div>
          <p className="hint">Find the one guest who wasn't really invited.</p>
        </>
      ) : (
        <>
          <div className="role-tag outsider">You're the outsider</div>
          <div className="role-line dim">You don't know what this party is.</div>
          <p className="hint">Blend in, figure out the party, and don't get caught.</p>
        </>
      )}
    </div>
  );
}

// Whoever is currently asking is the "active" guest. When it's the human's turn
// to ask that's us; otherwise it's the asker of the in-flight exchange (the
// `pending` turn covers both AIs talking among themselves and an AI questioning
// the human). Only meaningful while questioning — nobody asks during vote/reveal.
function activeAsker(game: GameView): number | undefined {
  if (game.phase !== "questioning") return undefined;
  if (game.awaiting?.kind === "question") return game.humanId;
  if (game.pending) return game.pending.asker;
  if (game.awaiting?.kind === "answer") return game.awaiting.asker;
  return undefined;
}

// Every guest at the party, the human included (shown as "You"). The active
// asker is marked. Side is only set at reveal (view() filters it before then),
// so we tag it only when it's available.
function GuestList({ game, activeId }: { game: GameView; activeId?: number }) {
  // In Bartender mode the human runs the bar rather than mingling, so they're
  // not one of the guests being scrutinized.
  const guests =
    game.mode === "bartender" ? game.players.filter((p) => !p.isHuman) : game.players;
  // While voting, surface each guest as they cast a vote (and who's mid-vote).
  // Their target is masked to "***" until the reveal, so seeing who has voted
  // can't tip the player's own choice; the player always sees their own pick.
  const voting = game.phase === "voting";
  const nameOf = (id: number) => game.players.find((p) => p.id === id)?.name ?? "?";
  const voteOf = (id: number) => game.votes?.find((v) => v.voter === id);

  // Nudge the guest who just picked up the asking baton, and pop each vote in as
  // it's cast. Both hooks live above the early return so hook order stays stable.
  const root = useRef<HTMLDivElement | null>(null);
  useGSAP(
    () => {
      if (reduceMotion() || activeId == null) return;
      gsap.from(".guests-list li.is-active", {
        x: -6,
        scale: 0.97,
        duration: DUR.fast,
        ease: EASE,
        transformOrigin: "left center",
      });
    },
    { scope: root, dependencies: [activeId] },
  );
  const shownVotes = useRef(0);
  const voteCount = game.votes?.length ?? 0;
  useGSAP(
    () => {
      if (reduceMotion()) {
        shownVotes.current = voteCount;
        return;
      }
      const cast = gsap.utils.toArray<HTMLElement>(".guests-list .guest-vote");
      const fresh = cast.slice(shownVotes.current);
      shownVotes.current = voteCount;
      if (fresh.length === 0) return;
      gsap.from(fresh, { opacity: 0, x: 8, duration: DUR.fast, ease: EASE });
    },
    { scope: root, dependencies: [voteCount] },
  );

  if (guests.length === 0) return null;
  return (
    <div className="guests" ref={root}>
      <div className="guests-title">Guests ({guests.length})</div>
      <ul className="guests-list">
        {guests.map((p) => {
          const vote = voting ? voteOf(p.id) : undefined;
          const isVoting = voting && game.thinking === p.id;
          const cls = [
            p.isHuman ? "is-you" : "",
            p.side ? `is-${p.side}` : "",
            p.id === activeId ? "is-active" : "",
          ]
            .filter(Boolean)
            .join(" ");
          return (
            <li key={p.id} className={cls}>
              <span className="guest-name">{p.isHuman ? "You" : p.name}</span>
              {p.id === activeId && <span className="guest-flag">asking…</span>}
              {isVoting && <span className="guest-flag">voting…</span>}
              {vote && (
                <span className="guest-vote">
                  → {vote.target != null ? nameOf(vote.target) : "***"}
                </span>
              )}
              {p.side && <span className="guest-side">{p.side === "uninvited" ? "outsider" : "invited"}</span>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// Each guest gets a stable identity color so the conversation is scannable at a
// glance (the old log painted every name the same gold). The human keeps the
// fixed --player blue; the AIs are dealt distinct hues by seat order. We avoid
// the invited-green / outsider-red here — those carry meaning at reveal and
// shouldn't leak into idle chatter.
const PALETTE = [
  "#f5b53d", "#b98cf0", "#54d6c2", "#f08fb0",
  "#e8915a", "#9bd35a", "#d36fc4", "#6fb6f0",
];

function colorFor(game: GameView, id: number): string {
  const p = game.players.find((x) => x.id === id);
  if (p?.isHuman) return "var(--player)";
  const seat = game.players.filter((x) => !x.isHuman).findIndex((x) => x.id === id);
  return PALETTE[(seat < 0 ? 0 : seat) % PALETTE.length];
}

function labelFor(game: GameView, id: number): string {
  const p = game.players.find((x) => x.id === id);
  if (!p) return "?";
  return p.isHuman ? "You" : p.name;
}

// A colored avatar + name, tinted with the guest's identity color via --c.
function Chip({ game, id }: { game: GameView; id: number }) {
  const label = labelFor(game, id);
  return (
    <span className="chip" style={{ "--c": colorFor(game, id) } as CSSProperties}>
      <span className="chip-av">{label.charAt(0).toUpperCase()}</span>
      <span className="chip-name">{label}</span>
    </span>
  );
}

function TranscriptLog({ game }: { game: GameView }) {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const endRef = useRef<HTMLDivElement | null>(null);
  // Keep the newest line in view as turns land and as the pending exchange
  // appears/updates (question shown, then answer being composed).
  const pending = game.pending;
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [game.transcript.length, pending?.asker, pending?.question, game.thinking]);

  // Slide each newly-landed exchange into the log. We remember how many entries
  // have already played (`shown`) and animate only the ones added since — older
  // lines stay settled (useGSAP doesn't revert on dependency change).
  const shown = useRef(0);
  useGSAP(
    () => {
      if (reduceMotion()) {
        shown.current = game.transcript.length;
        return;
      }
      const settled = gsap.utils.toArray<HTMLElement>(".tlog-entry:not(.pending)");
      const fresh = settled.slice(shown.current);
      shown.current = game.transcript.length;
      if (fresh.length === 0) return;
      gsap.from(fresh, {
        opacity: 0,
        y: 12,
        duration: DUR.base,
        ease: EASE,
        stagger: 0.06,
      });
    },
    { scope: bodyRef, dependencies: [game.transcript.length] },
  );

  // The "someone is thinking" bubble eases in each time a new exchange opens.
  useGSAP(
    () => {
      if (reduceMotion() || !pending) return;
      gsap.from(".tlog-entry.pending", {
        opacity: 0,
        y: 8,
        duration: DUR.fast,
        ease: EASE,
      });
    },
    { scope: bodyRef, dependencies: [pending?.asker, pending != null] },
  );

  return (
    <div className="tlog">
      <div className="tlog-title">
        Conversation
        {game.phase === "voting" && <span className="tlog-phase">Voting</span>}
      </div>
      <div className="tlog-body" ref={bodyRef}>
        {game.transcript.length === 0 && !pending && (
          <div className="dim">No one has spoken yet.</div>
        )}
        {game.transcript.map((t, i) => (
          <div
            className="tlog-entry"
            key={i}
            style={{ "--asker": colorFor(game, t.asker) } as CSSProperties}
          >
            <div className="xc-head">
              <Chip game={game} id={t.asker} />
              <span className="xc-arrow">→</span>
              <Chip game={game} id={t.target} />
            </div>
            <div className="xc-q">{t.question}</div>
            <div className="xc-div" />
            <div className="xc-a">
              <span className="xc-reply" style={{ color: colorFor(game, t.target) }}>↳</span>
              <span>{t.answer}</span>
            </div>
            {/* Reserved: per-card actions (e.g. flag a guest) mount here. */}
          </div>
        ))}
        {pending && (
          <div
            className="tlog-entry pending"
            style={{ "--asker": colorFor(game, pending.asker) } as CSSProperties}
          >
            <div className="xc-head">
              <Chip game={game} id={pending.asker} />
              <span className="xc-arrow">→</span>
              <Chip game={game} id={pending.target} />
            </div>
            <div className="xc-q">{pending.question ?? <Typing />}</div>
            {pending.question != null && (
              <>
                <div className="xc-div" />
                <div className="xc-a">
                  <span className="xc-reply" style={{ color: colorFor(game, pending.target) }}>↳</span>
                  {game.thinking === pending.target ? (
                    <Typing />
                  ) : (
                    <span className="dim">…</span>
                  )}
                </div>
              </>
            )}
          </div>
        )}
        <div ref={endRef} />
      </div>
    </div>
  );
}

// A small animated "…" shown wherever an agent is mid-generation — both while a
// question is being composed and while its answer is.
function Typing() {
  return (
    <span className="typing">
      <span className="typing-dots" aria-hidden="true">
        <i />
        <i />
        <i />
      </span>
    </span>
  );
}

function ThoughtsPanel({ game, thoughts }: { game: GameView; thoughts: AgentActivity[] }) {
  const [open, setOpen] = useState(false);
  const name = (id: number) => game.players.find((p) => p.id === id)?.name ?? "?";
  return (
    <div className="thoughts">
      <button className="thoughts-toggle" onClick={() => setOpen((o) => !o)}>
        {open ? "▾" : "▸"} AI thoughts ({thoughts.length})
      </button>
      {open && (
        <div className="thoughts-body">
          {thoughts.map((t, i) => (
            <div className="thought" key={i}>
              <span className="who">{name(t.playerId)}</span>: {t.text}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function InputBar({ game, parties }: { game: GameView; parties: Party[] }) {
  const aw = game.awaiting;
  const others = game.players.filter((p) => p.id !== game.humanId);
  // In the chain you can't immediately ask back whoever just asked you.
  const lastTurn = game.transcript[game.transcript.length - 1];
  const justAskedMe =
    lastTurn && lastTurn.target === game.humanId ? lastTurn.asker : undefined;
  const questionTargets = others.filter((p) => p.id !== justAskedMe);
  const [text, setText] = useState("");
  const [target, setTarget] = useState<number>(others[0]?.id ?? 1);
  const [sent, setSent] = useState(false);
  const [guessing, setGuessing] = useState(false);
  const [guess, setGuess] = useState("");

  // Reset controls whenever the backend asks for something new.
  useEffect(() => {
    setText("");
    setSent(false);
    setGuessing(false);
    setGuess("");
    if (aw?.kind === "question") setTarget(aw.target);
  }, [aw]);

  // Slide the bar up whenever the prompt itself changes (handing the turn to the
  // human, or dropping back to "waiting"). There's only ever one .inputbar, so a
  // global selector is fine; we key off the prompt identity, not every re-render,
  // so consecutive questions don't jitter the bar.
  const barKey = aw ? aw.kind : "waiting";
  useGSAP(
    () => {
      if (reduceMotion()) return;
      gsap.from(".inputbar", { y: 16, opacity: 0, duration: DUR.base, ease: EASE });
    },
    { dependencies: [barKey] },
  );

  if (!aw) {
    const thinker =
      game.thinking != null
        ? game.players.find((p) => p.id === game.thinking)?.name
        : undefined;
    // During the vote it's "voting", not the generic "thinking/talking".
    const voting = game.phase === "voting";
    const verb = voting ? "voting" : "thinking";
    const idle = voting ? "The guests are voting…" : "The guests are talking…";
    // Bartender mode: the human only listens during the chain, so let them end
    // it whenever they've heard enough and go straight to the vote.
    const canCallVote = game.mode === "bartender" && game.phase === "questioning";
    return (
      <div className="inputbar waiting">
        <span className="spinner" /> {thinker ? `${thinker} is ${verb}…` : idle}
        {canCallVote && (
          <button
            className="primary"
            disabled={sent}
            onClick={async () => {
              setSent(true);
              await invoke("call_vote");
            }}
          >
            Call the vote
          </button>
        )}
      </div>
    );
  }

  const canGuess = !game.humanInvited;

  const submitGuess = async () => {
    if (!guess.trim() || sent) return;
    setSent(true);
    await invoke("submit_location_guess", { text: guess.trim() });
  };

  if (guessing) {
    // We submit the occasion — the distinctive part the backend matches on (the
    // venue alone is generic and shared across parties).
    return (
      <div className="inputbar">
        <div className="bar-label">Which party is it? Pick right and you win, wrong and you're caught.</div>
        <div className="loclist">
          {parties.map((p) => (
            <button
              key={`${p.location}|${p.occasion}`}
              className={`locopt ${p.occasion === guess ? "locopt-on" : ""}`}
              onClick={() => setGuess(p.occasion)}
            >
              <span className="party-occ">{p.occasion}</span>
              <span className="party-loc">{p.location}</span>
            </button>
          ))}
        </div>
        <div className="bar-row">
          <button className="primary" disabled={sent || !guess} onClick={submitGuess}>
            Guess
          </button>
          <button className="ghost" onClick={() => setGuessing(false)}>
            Back
          </button>
        </div>
      </div>
    );
  }

  const guessBtn = canGuess && (
    <button className="ghost guess" onClick={() => setGuessing(true)}>
      Guess the party
    </button>
  );

  if (aw.kind === "question") {
    const ask = async () => {
      if (!text.trim() || sent) return;
      setSent(true);
      await invoke("submit_question", { target, text: text.trim() });
    };
    return (
      <div className="inputbar">
        <div className="bar-label">Ask a question to:</div>
        <TargetPicker others={questionTargets} value={target} onPick={setTarget} />
        <div className="bar-row">
          <input
            className="text"
            autoFocus
            placeholder="Ask something only a real guest could answer…"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ask()}
          />
          <button className="primary" disabled={sent || !text.trim()} onClick={ask}>
            Ask
          </button>
          {guessBtn}
        </div>
      </div>
    );
  }

  if (aw.kind === "answer") {
    const answer = async () => {
      if (!text.trim() || sent) return;
      setSent(true);
      await invoke("submit_answer", { text: text.trim() });
    };
    const askerName = game.players.find((p) => p.id === aw.asker)?.name ?? "Someone";
    return (
      <div className="inputbar">
        <div className="bar-label">
          <strong>{askerName}</strong> asks you: “{aw.question}”
        </div>
        <div className="bar-row">
          <input
            className="text"
            autoFocus
            placeholder="Answer like you belong here…"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && answer()}
          />
          <button className="primary" disabled={sent || !text.trim()} onClick={answer}>
            Answer
          </button>
          {guessBtn}
        </div>
      </div>
    );
  }

  // vote
  const vote = async () => {
    if (sent) return;
    setSent(true);
    await invoke("submit_vote", { target });
  };
  return (
    <div className="inputbar">
      <div className="bar-label">Who is the outsider? Cast your vote:</div>
      <TargetPicker others={others} value={target} onPick={setTarget} />
      <div className="bar-row">
        <button className="primary" disabled={sent} onClick={vote}>
          Vote out {game.players.find((p) => p.id === target)?.name}
        </button>
        {guessBtn}
      </div>
    </div>
  );
}

function TargetPicker({
  others,
  value,
  onPick,
}: {
  others: { id: number; name: string }[];
  value: number;
  onPick: (id: number) => void;
}) {
  return (
    <div className="targetpick">
      {others.map((p) => (
        <button
          key={p.id}
          className={`pill ${p.id === value ? "pill-on" : ""}`}
          onClick={() => onPick(p.id)}
        >
          {p.name}
        </button>
      ))}
    </div>
  );
}
