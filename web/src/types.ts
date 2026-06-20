// Mirrors the Rust serde structs (camelCase). Status comes over the "status"
// event; the game snapshot over "game-state"; per-action lines over
// "agent-activity". Keep these in sync with src-tauri/src/{inference,game,
// orchestrator}.rs.

export type Phase = "idle" | "downloading" | "loading" | "ready" | "error";

export interface ServerStatus {
  phase: Phase;
  modelReady: boolean;
  modelName: string;
  progress: number;
  downloadedBytes: number;
  totalBytes: number;
  message: string;
  error?: string;
}

export type GamePhase = "questioning" | "voting" | "reveal";
export type Side = "invited" | "uninvited";
// How the human plays. "bartender" watches the Q&A and is the sole voter.
export type GameMode = "invited" | "outsider" | "bartender";

export interface PlayerView {
  id: number;
  name: string;
  isHuman: boolean;
  side?: Side; // revealed only at the end
}

export interface TurnView {
  asker: number;
  target: number;
  question: string;
  answer: string;
}

export type Awaiting =
  | { kind: "question"; target: number }
  | { kind: "answer"; asker: number; question: string }
  | { kind: "vote" };

// The in-flight exchange, shown before its answer exists. `question` is absent
// while the asker is still composing it.
export interface PendingTurn {
  asker: number;
  target: number;
  question?: string;
}

export interface VoteView {
  voter: number;
  target: number | null; // null while masked pre-reveal (rendered "***")
}

export interface GuessView {
  player: number;
  guess: string;
  correct: boolean;
}

// A party on the public board: the occasion (event) and its location (venue).
export interface Party {
  occasion: string;
  location: string;
}

export interface GameView {
  phase: GamePhase;
  players: PlayerView[];
  humanId: number;
  humanInvited: boolean;
  mode: GameMode;
  partyRole?: string;
  occasion?: string; // the event; shown to invited players and at reveal
  location?: string; // the venue; shown to invited players and at reveal
  round: number;
  roundsTotal: number;
  transcript: TurnView[];
  awaiting?: Awaiting;
  pending?: PendingTurn; // question/answer in flight, shown before the answer
  thinking?: number; // id of the AI currently generating (question/answer/vote)
  votes?: VoteView[];
  uninvitedId?: number;
  winner?: Side;
  guess?: GuessView;
}

export type ActivityKind = "question" | "answer" | "vote" | "thinking" | "guess";

export interface AgentActivity {
  playerId: number;
  kind: ActivityKind;
  text: string;
  targetId?: number;
}

export interface GameConfig {
  mode: GameMode;
  aiCount: number;
}
