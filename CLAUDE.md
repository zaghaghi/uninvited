# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Uninvited is a **local-LLM party deduction game** (SpyFall reskinned as a party). It's a Tauri v2 desktop app: a Rust backend that runs a Gemma model in-process via llama.cpp, and a React frontend. The game happens **before** the party: everyone is chatting about an upcoming event nobody has gone to yet. All the guests were "invited" and know what the party is — except one "outsider" who wasn't invited and doesn't know it. Invited guests question each other to find the outsider, who blends in and tries to deduce (and guess) the party. (Because it hasn't happened yet, agents should talk about getting ready and looking forward to it, never as if they're already there — and the outsider guessing the party is the whole point, which only works while the party is still unknown to them.)

## Commands

Run from the repo root unless noted. First app launch downloads the model (~3.35 GB) to `~/.xaghoul-games/brains/`.

```bash
npm install                 # root: installs the Tauri CLI only
npm --prefix web install    # web deps (React, Vite)
npm run dev                 # `tauri dev`: launches Vite + the desktop app
npm run build               # `tauri build`: production bundle

# Rust (from src-tauri/)
cargo test                          # all backend unit tests
cargo test scenarios                # tests in one module (substring match)
cargo test parse_uninvited_turn     # a single test by name
cargo check                         # fast type-check
cargo run --release --example headless -p uninvited   # boot the engine + one generation, no GUI

# Frontend (from web/)
npm run typecheck           # tsc --noEmit (strict; noUnusedLocals/Parameters on)
npm run build               # tsc -b && vite build
```

There is no test runner for the frontend; `npm run typecheck` is the check. Always run **both** `cargo check` and (from `web/`) `npm run typecheck` after changes — the two halves are connected only by hand-mirrored types (see below).

### Env knobs (read at runtime by the backend)

- `UNINVITED_THINKING=false` — disable the model's reasoning channel (default on).
- `UNINVITED_ROUNDS=3` — questioning rounds before the vote (default 2).
- `UNINVITED_DEBUG_THINKING=1` — surface each AI's hidden reasoning in the in-game "AI thoughts" panel (a spoiler; dev only).
- `UNINVITED_DEBUG_RAW=1` — print prompts and raw model output to stderr.

## Architecture

### The single source of truth is the backend `GameSession`

`game.rs` holds `GameSession`, which contains the **full secret truth** (who's the outsider, the actual party, roles). The frontend never sees this directly. Instead:

- `GameSession::view()` produces a `GameView` — a **secret-filtered snapshot**. The party is included only when the human is invited or the game has reached `Phase::Reveal`; the outsider's identity, votes, and winner appear only at reveal. **When adding fields, do the filtering in `view()`** — anything you put on `GameView` unconditionally is visible to the outsider and will leak the game.
- The frontend is a pure render of whatever `GameView`/events arrive; it holds no game rules.

### The orchestrator is the game loop (one async task per game)

`orchestrator.rs::run_game` is spawned once per game (`commands.rs::start_game`). It owns the `GameSession`, walks the SpyFall flow — round-robin questioning where **the answerer becomes the next asker (the chain)**, then a vote — and:

- Calls the model sequentially for each AI action via `engine.generate(system, user, max_tokens)`.
- Pauses for the human by awaiting on an mpsc channel; the human's Tauri commands (`submit_question`/`submit_answer`/`submit_vote`/`submit_location_guess`) push `HumanInput` into it. The typed receivers (`recv_for_question`/`_answer`/`_vote`) accept the variant for the current phase **plus a guess at any time** (the outsider can guess on their turn, when answering, or when voting), and ignore stray input.
- Emits two Tauri events after transitions: **`game-state`** (a `GameView`) and **`agent-activity`** (one line per AI action, used to animate the scene and the debug thoughts panel).
- A correct guess → outsider wins; a wrong guess → invited win. Either way the game ends immediately (`finish_with_guess`).

### Inference: one model, one worker thread, stateless prompts

`inference.rs` wraps `llama-cpp-2` (Metal). All llama.cpp objects live on a **single dedicated worker thread** (they aren't `Sync`); the async orchestrator talks to it over a channel with oneshot replies. Key facts:

- Every generation is a **fresh prompt from `n_past=0`** — there is no cross-turn KV reuse. The game rebuilds the full public transcript into the `user` string each turn (`agents.rs::transcript_block`), so every agent reasons over the same shared history.
- "Thinking" mode (default on) renders the model's **baked Gemma chat template** with `enable_thinking=true` via minijinja, then `split_thinking` separates the reasoning channel from the answer. If the template render fails it falls back to the manual Gemma prompt (`build_gemma_prompt`) with no reasoning. The minijinja env patches in a `.get()` method because the official template calls Python dict methods.
- `brains.rs` is the model catalog + on-demand HuggingFace downloader. There is one active brain (`ACTIVE = "gemma-4-e2b"`), cached in `~/.xaghoul-games/brains/`.

### Agents = prompt construction, nothing else

`agents.rs` builds the `(system, user)` pair for each AI action and parses replies. The **system** string is the player's private role briefing (invited briefings embed the party as an upcoming event and warn never to reveal it — not the name, occasion, who/what it's for, or venue, only oblique hints; the outsider briefing says it wasn't invited and doesn't know the party). Both briefings frame the chatter as happening *before* the party, so agents don't talk as if they're already there. The **user** string is the shared transcript plus this turn's task. This module knows nothing about Tauri — the orchestrator wires it to the engine. Replies are normalized through `clean_line` (small models ramble); the outsider's turn is parsed for a `GUESS:`/`ASK:` prefix.

### The deck

`scenarios.rs` bundles `scenarios.json` into the binary (`include_str!`). Each scenario is a hidden party split into an `occasion` (the event) and a `location` (the venue), plus `aliases` (for lenient guess matching via `guess_matches`, which matches on the **occasion + aliases** — the bare venue is generic and shared across parties, so it isn't a guess candidate) and a pool of `roles` dealt to invited players. `draw()` picks one at random; `all_parties()` returns the full public board (`Party { occasion, location }`) shown to all players.

## Frontend notes

- `web/src/useGame.ts` subscribes to the three backend events (`status`, `game-state`, `agent-activity`) and holds all app state. `App.tsx` switches screens off `status.phase` and `game.phase`.
- `web/src/types.ts` **hand-mirrors the Rust serde structs** (camelCase via `#[serde(rename_all = "camelCase")]`). When you change a `GameView`/`Status`/`AgentActivity` field or add a Tauri command, update both sides.
- New Tauri commands must be registered in `lib.rs`'s `invoke_handler![...]` to be callable from the UI.
- The UI is plain React/HTML (`components/`): the `GameScreen` puts the conversation transcript front and center, with the role card, the public party board, and the (debug) thoughts panel in a slim sidebar. The backend still emits `round`/`roundsTotal`, but the UI no longer surfaces a round counter (the chain mechanic makes "rounds" imperceptible to the player).

## Conventions

- Comments here explain **why** (the secret-filtering, the chain rule, the single-worker constraint, the stateless-prompt design) rather than what. Match that density and intent.
- The backend is the rules engine; the frontend renders. Don't push game logic into React.
