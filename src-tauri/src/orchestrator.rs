// The game loop. Spawned once per game as an async task, it walks the SpyFall
// flow — round-robin questioning, then a vote — calling the single local model
// sequentially for each AI action and pausing for the human via an mpsc
// channel. It owns the `GameSession` (the full truth) and emits two events:
//   - "game-state":   a secret-filtered snapshot after every transition
//   - "agent-activity": one line per AI action, for the scene + (debug) thoughts
//
// A guess ends the game immediately: right -> the uninvited wins, wrong -> the
// invited win (guessing is a real gamble, as in SpyFall).

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::agents::{self, UninvitedAction};
use crate::game::{Awaiting, GameConfig, GameSession, Phase, Side, HUMAN_ID};
use crate::inference::{self, Engine};

/// Input the human submits via Tauri commands, delivered over the game channel.
#[derive(Clone, Debug)]
pub enum HumanInput {
    Question { target: u8, text: String },
    Answer { text: String },
    Vote { target: u8 },
    GuessLocation { text: String },
    /// Bartender mode only: end the eavesdropping early and open the vote.
    CallVote,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    pub player_id: u8,
    pub kind: &'static str, // "question" | "answer" | "vote" | "thinking" | "guess"
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<u8>,
}

/// Force the human back into the chain if they've gone this many turns without
/// asking or answering. The random chain can otherwise leave them idle for a
/// long stretch; this keeps them in the loop within roughly every third turn.
const HUMAN_IDLE_LIMIT: usize = 2;

fn gen_tokens() -> usize {
    // Thinking burns most tokens on reasoning before the one-line answer.
    if inference::thinking_enabled() {
        1024
    } else {
        128
    }
}

fn debug_thinking() -> bool {
    std::env::var("UNINVITED_DEBUG_THINKING").is_ok()
}

fn emit_state(app: &AppHandle, s: &GameSession) {
    let _ = app.emit("game-state", s.view());
}

fn emit_activity(app: &AppHandle, a: AgentActivity) {
    let _ = app.emit("agent-activity", a);
}

/// Surface an AI's hidden reasoning only when explicitly enabled — otherwise it
/// would spoil the deduction (an outsider's thoughts give the game away).
fn emit_thinking(app: &AppHandle, player_id: u8, thinking: &Option<String>) {
    if debug_thinking() {
        if let Some(t) = thinking {
            emit_activity(
                app,
                AgentActivity {
                    player_id,
                    kind: "thinking",
                    text: t.clone(),
                    target_id: None,
                },
            );
        }
    }
}

/// Pick someone for `asker` to question. Excludes `asker`, and avoids `exclude`
/// (the person who just asked `asker` — you can't immediately ask back) unless
/// that would leave no one. With 3+ players there's always a valid choice. In
/// Bartender mode the human is a silent spectator, so they're never a target.
fn pick_target(s: &GameSession, asker: u8, exclude: Option<u8>, bartender: bool) -> u8 {
    use rand::seq::SliceRandom;
    let mut cands: Vec<u8> = s
        .other_ids(asker)
        .into_iter()
        .filter(|&id| !(bartender && id == HUMAN_ID))
        .collect();
    if let Some(ex) = exclude {
        let filtered: Vec<u8> = cands.iter().copied().filter(|&id| id != ex).collect();
        if !filtered.is_empty() {
            cands = filtered;
        }
    }
    *cands
        .choose(&mut rand::thread_rng())
        .expect("at least one other player")
}

/// The first asker is chosen at random. In Bartender mode the human watches
/// rather than plays, so only the AI guests can open the conversation.
fn pick_first_asker(s: &GameSession, bartender: bool) -> u8 {
    use rand::seq::SliceRandom;
    let ids: Vec<u8> = s
        .players
        .iter()
        .map(|p| p.id)
        .filter(|&id| !(bartender && id == HUMAN_ID))
        .collect();
    *ids.choose(&mut rand::thread_rng()).unwrap_or(&HUMAN_ID)
}

/// Apply a location guess and end the game. Returns having set the winner and
/// the reveal phase; the caller should stop the loop.
fn finish_with_guess(app: &AppHandle, s: &mut GameSession, player: u8, guess: String) {
    let correct = s.record_guess(player, guess);
    if !correct {
        s.winner = Some(Side::Invited);
        s.phase = Phase::Reveal;
    }
    s.awaiting = None;
    s.pending = None;
    s.thinking = None;
    emit_state(app, s);
}

/// The entry point: run one full game to its reveal.
pub async fn run_game(
    engine: Engine,
    app: AppHandle,
    mut rx: UnboundedReceiver<HumanInput>,
    cfg: GameConfig,
) {
    let mut s = GameSession::new(&cfg);
    // In Bartender mode the human only watches and votes, so the Q&A chain runs
    // among the AI guests alone.
    let bartender = s.bartender_mode();
    emit_state(&app, &s);

    // -- questioning: whoever answers becomes the next asker (SpyFall chain) -
    // Count only the guests who actually take part, so the chain length scales
    // the same whether or not the human is in it.
    let qa = if bartender {
        s.players.len() - 1
    } else {
        s.players.len()
    };
    let total_questions = s.rounds_total as usize * qa;
    let mut asker_id = pick_first_asker(&s, bartender);
    let mut prev_asker: Option<u8> = None;
    // Turns since the human last asked or answered; bounds how long the random
    // chain may leave them out (see HUMAN_IDLE_LIMIT). Unused in Bartender mode.
    let mut human_idle = 0usize;

    for q in 0..total_questions {
        s.round = (q / qa) as u8 + 1;
        // Bartender mode: between turns, honor a "call the vote" request to end
        // the eavesdropping early. Generation runs on the worker thread, so this
        // is the natural place to check.
        if bartender {
            let mut called = false;
            loop {
                match rx.try_recv() {
                    Ok(HumanInput::CallVote) => {
                        called = true;
                        break;
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            if called {
                break;
            }
        }
        {
            let mut target_id = pick_target(&s, asker_id, prev_asker, bartender);
            // If the human has been idle too long, force the AI to question them
            // so they're never stuck outside the conversation for long. (Never in
            // Bartender mode — the human isn't part of the conversation.)
            if !bartender && asker_id != HUMAN_ID && human_idle >= HUMAN_IDLE_LIMIT {
                target_id = HUMAN_ID;
            }
            // Clear last turn's in-flight state; the branches below repopulate it
            // as soon as there's a question to show or an AI to mark as thinking.
            s.pending = None;
            s.thinking = None;
            let asker = s.player(asker_id).clone();

            // 1) Produce the question (or an uninvited guess).
            let question: String;
            if asker.is_human {
                s.awaiting = Some(Awaiting::Question { target: target_id });
                emit_state(&app, &s);
                match recv_for_question(&mut rx).await {
                    QResult::Closed => return,
                    QResult::Guess(g) => {
                        finish_with_guess(&app, &mut s, HUMAN_ID, g);
                        return;
                    }
                    QResult::Question { target, text } => {
                        if target != asker_id && (target as usize) < s.players.len() {
                            target_id = target;
                        }
                        question = agents::clean_line(&text);
                    }
                }
                // The human has asked — drop the ask input now, so it doesn't
                // linger (disabled but visible) through the slow answer
                // generation before the next state emit clears it.
                s.awaiting = None;
            } else if asker.role.side() == Side::Uninvited {
                // Show the asker composing before the (slow) generation runs.
                s.pending = Some(crate::game::PendingTurn {
                    asker: asker_id,
                    target: target_id,
                    question: None,
                });
                s.thinking = Some(asker_id);
                emit_state(&app, &s);
                let (sys, user) = agents::uninvited_turn_prompt(&s, &asker, target_id);
                let reply = match engine.generate(&sys, &user, gen_tokens()).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[uninvited] uninvited turn failed: {e}");
                        asker_id = pick_target(&s, asker_id, prev_asker, bartender);
                        continue;
                    }
                };
                emit_thinking(&app, asker_id, &reply.thinking);
                match agents::parse_uninvited_turn(&reply.text) {
                    UninvitedAction::Guess(g) => {
                        emit_activity(
                            &app,
                            AgentActivity {
                                player_id: asker_id,
                                kind: "guess",
                                text: g.clone(),
                                target_id: None,
                            },
                        );
                        finish_with_guess(&app, &mut s, asker_id, g);
                        return;
                    }
                    UninvitedAction::Ask(q) => question = q,
                }
            } else {
                // Show the asker composing before the (slow) generation runs.
                s.pending = Some(crate::game::PendingTurn {
                    asker: asker_id,
                    target: target_id,
                    question: None,
                });
                s.thinking = Some(asker_id);
                emit_state(&app, &s);
                let (sys, user) = agents::ask_prompt(&s, &asker, target_id);
                let reply = match engine.generate(&sys, &user, gen_tokens()).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[uninvited] ask failed: {e}");
                        asker_id = pick_target(&s, asker_id, prev_asker, bartender);
                        continue;
                    }
                };
                emit_thinking(&app, asker_id, &reply.thinking);
                question = agents::clean_line(&reply.text);
            }

            if question.trim().is_empty() {
                asker_id = pick_target(&s, asker_id, prev_asker, bartender);
                continue;
            }
            emit_activity(
                &app,
                AgentActivity {
                    player_id: asker_id,
                    kind: "question",
                    text: question.clone(),
                    target_id: Some(target_id),
                },
            );

            // The question now exists — surface it right away (before the answer
            // is produced) so the player reads it while the answer is generated.
            s.pending = Some(crate::game::PendingTurn {
                asker: asker_id,
                target: target_id,
                question: Some(question.clone()),
            });
            s.thinking = None;

            // 2) Get the answer from the target.
            let target = s.player(target_id).clone();
            let answer: String;
            if target.is_human {
                s.awaiting = Some(Awaiting::Answer {
                    asker: asker_id,
                    question: question.clone(),
                });
                emit_state(&app, &s);
                match recv_for_answer(&mut rx).await {
                    AResult::Closed => return,
                    AResult::Guess(g) => {
                        finish_with_guess(&app, &mut s, HUMAN_ID, g);
                        return;
                    }
                    AResult::Answer(text) => answer = agents::clean_line(&text),
                }
            } else {
                // Question is already on screen; show the target composing.
                s.thinking = Some(target_id);
                emit_state(&app, &s);
                let (sys, user) = agents::answer_prompt(&s, &target, asker_id, &question);
                match engine.generate(&sys, &user, gen_tokens()).await {
                    Ok(reply) => {
                        emit_thinking(&app, target_id, &reply.thinking);
                        answer = agents::clean_line(&reply.text);
                    }
                    Err(e) => {
                        eprintln!("[uninvited] answer failed: {e}");
                        answer = "…".to_string();
                    }
                }
            }

            s.awaiting = None;
            s.pending = None;
            s.thinking = None;
            s.transcript.push(crate::game::Turn {
                asker: asker_id,
                target: target_id,
                question,
                answer: answer.clone(),
            });
            emit_activity(
                &app,
                AgentActivity {
                    player_id: target_id,
                    kind: "answer",
                    text: answer,
                    target_id: Some(asker_id),
                },
            );
            emit_state(&app, &s);

            // The human counts as engaged whenever they asked or answered.
            if asker_id == HUMAN_ID || target_id == HUMAN_ID {
                human_idle = 0;
            } else {
                human_idle += 1;
            }
            // The answerer becomes the next asker (SpyFall chaining).
            prev_asker = Some(asker_id);
            asker_id = target_id;
        }
    }

    // -- voting ------------------------------------------------------------
    // Normally the other guests vote while the human is still deciding. The
    // engine is a single worker, so the AI votes generate one at a time; between
    // each we drain any vote the human has already submitted (without blocking
    // the AIs). Every AI pick is masked to "***" in the view until the reveal,
    // so watching who has voted can't sway the human's own choice. In Bartender
    // mode the guests don't vote at all — only the human does (see below).
    s.phase = Phase::Voting;
    s.pending = None;
    s.thinking = None;
    s.awaiting = Some(Awaiting::Vote);
    emit_state(&app, &s);

    if bartender {
        // Bartender mode: the guests don't vote — the human's single verdict
        // decides. `resolve_votes` reads that lone vote as a strict plurality,
        // so a correct call catches the outsider and a wrong one lets them go.
        match recv_for_vote(&mut rx).await {
            VResult::Closed => return,
            VResult::Guess(g) => {
                finish_with_guess(&app, &mut s, HUMAN_ID, g);
                return;
            }
            VResult::Vote(target) => {
                s.players[HUMAN_ID as usize].vote_for = Some(target);
            }
        }
        s.awaiting = None;
        emit_state(&app, &s);
    } else {
        let mut human_voted = false;
        let ai_voter_ids: Vec<u8> = s
            .players
            .iter()
            .filter(|p| !p.is_human)
            .map(|p| p.id)
            .collect();
        for voter_id in ai_voter_ids {
            // Pick up the human's vote (or last-ditch guess) if it has landed,
            // without waiting on it.
            if !human_voted {
                loop {
                    match rx.try_recv() {
                        Ok(HumanInput::Vote { target }) => {
                            s.players[HUMAN_ID as usize].vote_for = Some(target);
                            human_voted = true;
                            s.awaiting = None;
                            emit_state(&app, &s);
                            break;
                        }
                        Ok(HumanInput::GuessLocation { text }) => {
                            finish_with_guess(&app, &mut s, HUMAN_ID, text);
                            return;
                        }
                        Ok(_) => continue, // stray input meant for an earlier phase
                        Err(_) => break,   // nothing pending (or channel closed)
                    }
                }
            }

            let voter = s.player(voter_id).clone();
            // Mark the voter as voting while its (slow) decision is generated.
            s.thinking = Some(voter_id);
            emit_state(&app, &s);
            let (sys, user) = agents::vote_prompt(&s, &voter);
            let reply = match engine.generate(&sys, &user, gen_tokens()).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[uninvited] vote failed: {e}");
                    s.thinking = None;
                    continue;
                }
            };
            emit_thinking(&app, voter_id, &reply.thinking);
            let target = agents::parse_vote(&s, voter_id, &reply.text);
            s.players[voter_id as usize].vote_for = Some(target);
            s.thinking = None;
            emit_activity(
                &app,
                AgentActivity {
                    player_id: voter_id,
                    kind: "vote",
                    text: s.player(target).name.clone(),
                    target_id: Some(target),
                },
            );
            emit_state(&app, &s);
        }

        // The AIs have all voted; wait for the human if they still haven't.
        if !human_voted {
            match recv_for_vote(&mut rx).await {
                VResult::Closed => return,
                VResult::Guess(g) => {
                    finish_with_guess(&app, &mut s, HUMAN_ID, g);
                    return;
                }
                VResult::Vote(target) => {
                    s.players[HUMAN_ID as usize].vote_for = Some(target);
                }
            }
            s.awaiting = None;
            emit_state(&app, &s);
        }
    }

    // Everyone has voted. Hold a beat on the masked tally, then reveal who
    // voted for whom.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    s.resolve_votes();
    emit_state(&app, &s);
}

// -- typed receivers: accept the variant we're waiting on, plus a guess ----

enum QResult {
    Question { target: u8, text: String },
    Guess(String),
    Closed,
}
async fn recv_for_question(rx: &mut UnboundedReceiver<HumanInput>) -> QResult {
    loop {
        match rx.recv().await {
            None => return QResult::Closed,
            Some(HumanInput::Question { target, text }) => {
                return QResult::Question { target, text }
            }
            Some(HumanInput::GuessLocation { text }) => return QResult::Guess(text),
            _ => continue, // ignore stray input meant for another phase
        }
    }
}

enum AResult {
    Answer(String),
    Guess(String),
    Closed,
}
async fn recv_for_answer(rx: &mut UnboundedReceiver<HumanInput>) -> AResult {
    loop {
        match rx.recv().await {
            None => return AResult::Closed,
            Some(HumanInput::Answer { text }) => return AResult::Answer(text),
            Some(HumanInput::GuessLocation { text }) => return AResult::Guess(text),
            _ => continue,
        }
    }
}

enum VResult {
    Vote(u8),
    Guess(String),
    Closed,
}
async fn recv_for_vote(rx: &mut UnboundedReceiver<HumanInput>) -> VResult {
    loop {
        match rx.recv().await {
            None => return VResult::Closed,
            Some(HumanInput::Vote { target }) => return VResult::Vote(target),
            Some(HumanInput::GuessLocation { text }) => return VResult::Guess(text),
            _ => continue,
        }
    }
}
