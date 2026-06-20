// KV-cache strategy benchmark: an all-AI game played twice over the *identical*
// workload, comparing two prefill strategies.
//
//   1. "current"   — one shared LlamaContext, KV cache cleared before every
//                    generation (exactly what inference.rs does today). Each
//                    turn re-prefills the whole prompt (system + full transcript
//                    + task) from scratch.
//   2. "per-agent" — one persistent LlamaContext per player. The public
//                    transcript only ever grows (append-only), so an agent's
//                    prompt this turn shares a long token prefix with its prompt
//                    last turn. We keep that prefix in the agent's KV cache and
//                    prefill only the new suffix.
//
// Fair comparison is the hard part: `GameSession::new` randomizes the scenario/
// roles and generation is stochastic, so two independent games would diverge.
// Instead we *record then replay*: pass 1 runs the "current" strategy as a real
// game and records every model call as a Step{player, system, user, gen_len};
// pass 2 replays those exact prompts under the "per-agent" strategy, generating
// the same number of tokens each step. Identical prompts + identical generated
// tokens => the wall-clock delta is purely the prefill savings.
//
// This is gated behind the `bench-sim` feature so CI / `cargo test` never build
// or run it. Run it deliberately (release; first run downloads the model into
// the same cache the app uses, ~/.xaghoul-games/brains):
//
//   cargo run --release --example bench_kv -p uninvited --features bench-sim
//
// Knobs (env): BENCH_PLAYERS (AI count, default 3), BENCH_ROUNDS (questioning
// rounds, default 1), BENCH_MAXTOK (max new tokens/turn, default 256),
// BENCH_NCTX (context size per context, default 8192). Thinking follows the
// app's UNINVITED_THINKING (default on).

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use uninvited::agents::{self, UninvitedAction};
use uninvited::game::{GameConfig, GameMode, GameSession, Role, Turn, HUMAN_ID};
use uninvited::inference::{self, Engine, Phase};
use uninvited::{brains, scenarios};

/// Tokens per prefill batch — mirrors inference.rs (kept ≤ llama.cpp's default
/// n_batch so a long transcript can be fed in slices).
const PREFILL_CHUNK: usize = 512;
/// Fixed sampler seed so both strategies sample identically given identical
/// logits (the per-agent path reuses cached KV but the math is the same).
const SEED: u32 = 0xB17D;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

// -- knobs -----------------------------------------------------------------

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

struct Knobs {
    ai_count: u8,
    rounds: u8,
    max_tokens: usize,
    n_ctx: u32,
    thinking: bool,
}

fn knobs() -> Knobs {
    Knobs {
        ai_count: env_usize("BENCH_PLAYERS", 3).clamp(2, 8) as u8,
        rounds: env_usize("BENCH_ROUNDS", 1).max(1) as u8,
        max_tokens: env_usize("BENCH_MAXTOK", 256).max(1),
        n_ctx: env_usize("BENCH_NCTX", 8192).max(512) as u32,
        thinking: inference::thinking_enabled(),
    }
}

// -- metrics ---------------------------------------------------------------

#[derive(Default)]
struct Acc {
    generations: u64,
    prefill_tokens: u64,
    gen_tokens: u64,
    prefill_time: Duration,
    gen_time: Duration,
}

impl Acc {
    fn total(&self) -> Duration {
        self.prefill_time + self.gen_time
    }
}

/// One recorded model call: which player issued it, the exact (system, user)
/// prompt, and how many tokens it generated (so the replay matches decode work).
struct Step {
    player: u8,
    system: String,
    user: String,
    gen_len: usize,
}

// -- prompt + generation primitives ----------------------------------------

/// Build the prompt exactly as inference.rs would: the baked thinking template
/// when on, the manual Gemma prompt otherwise.
fn build_prompt(model: &LlamaModel, system: &str, user: &str, thinking: bool) -> String {
    if thinking {
        match inference::render_thinking_template(model, system, user) {
            Ok(p) => return p,
            Err(e) => eprintln!("[bench] thinking template failed ({e}); manual prompt"),
        }
    }
    inference::build_gemma_prompt(Some(system), &[("user".to_string(), user.to_string())])
}

fn tokenize(model: &LlamaModel, prompt: &str) -> Res<Vec<LlamaToken>> {
    Ok(model.str_to_token(prompt, AddBos::Always)?)
}

/// Length of the shared leading run of tokens — the part already cached and
/// reusable across an agent's consecutive prompts.
fn common_prefix_len(a: &[LlamaToken], b: &[LlamaToken]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Prefill `tokens[keep..]` into `ctx` (positions are absolute, so the kept
/// `[0, keep)` prefix lines up). Returns n_past (== tokens.len()). Times only
/// the decode work and adds the prefilled count to `acc`.
fn prefill(
    ctx: &mut LlamaContext,
    tokens: &[LlamaToken],
    keep: usize,
    acc: &mut Acc,
) -> Res<i32> {
    let n = tokens.len();
    let t0 = Instant::now();
    let mut batch = LlamaBatch::new(PREFILL_CHUNK, 1);
    let mut i = keep;
    while i < n {
        let end = (i + PREFILL_CHUNK).min(n);
        batch.clear();
        for j in i..end {
            batch.add(tokens[j], j as i32, &[0], j == n - 1)?;
        }
        ctx.decode(&mut batch)?;
        i = end;
    }
    acc.prefill_time += t0.elapsed();
    acc.prefill_tokens += (n - keep) as u64;
    Ok(n as i32)
}

struct Generated {
    text: String,
    produced: usize,
}

/// Decode tokens after a prefill. With `force = Some(k)` it generates exactly
/// `k` tokens (replay: match the recorded decode work, ignore stop signals);
/// with `force = None` it stops at EOG / a turn delimiter and returns the text
/// (recording the real game). Mirrors inference.rs's decode loop.
fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    mut n_past: i32,
    n_ctx: u32,
    max_new: usize,
    force: Option<usize>,
    thinking: bool,
    acc: &mut Acc,
) -> Res<Generated> {
    let t0 = Instant::now();
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::temp(0.8),
        LlamaSampler::dist(SEED),
    ]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut batch = LlamaBatch::new(1, 1);
    let mut out = String::new();
    let mut produced = 0usize;
    let want_text = force.is_none();
    let cap = force.unwrap_or(max_new);

    while produced < cap && (n_past as u32) < n_ctx - 1 {
        let token = sampler.sample(ctx, -1);
        sampler.accept(token);
        if want_text && model.is_eog_token(token) {
            break;
        }
        if want_text {
            let piece = model.token_to_piece(token, &mut decoder, thinking, None)?;
            out.push_str(&piece);
            if let Some(idx) = out.find("<end_of_turn>").or_else(|| out.find("<start_of_turn>")) {
                out.truncate(idx);
                break;
            }
        }
        batch.clear();
        batch.add(token, n_past, &[0], true)?;
        n_past += 1;
        ctx.decode(&mut batch)?;
        produced += 1;
    }

    acc.gen_time += t0.elapsed();
    acc.gen_tokens += produced as u64;
    acc.generations += 1;
    Ok(Generated { text: out, produced })
}

// -- the game flow ---------------------------------------------------------

fn pick_first_asker(s: &GameSession) -> u8 {
    use rand::seq::SliceRandom;
    let ids: Vec<u8> = s.players.iter().map(|p| p.id).collect();
    *ids.choose(&mut rand::thread_rng()).unwrap_or(&HUMAN_ID)
}

/// Pick someone for `asker` to question, avoiding `asker` and (if possible) the
/// person who just asked them — the same rule the orchestrator uses.
fn pick_target(s: &GameSession, asker: u8, exclude: Option<u8>) -> u8 {
    use rand::seq::SliceRandom;
    let mut cands = s.other_ids(asker);
    if let Some(ex) = exclude {
        let filtered: Vec<u8> = cands.iter().copied().filter(|&id| id != ex).collect();
        if !filtered.is_empty() {
            cands = filtered;
        }
    }
    *cands.choose(&mut rand::thread_rng()).expect("a target exists")
}

/// Pass 1: play a full all-AI game under the "current" strategy (single context,
/// full clear each turn), measuring it and recording every model call so pass 2
/// can replay the identical workload. Generation is real here — it builds the
/// transcript subsequent prompts read from.
fn play_and_record(
    model: &LlamaModel,
    backend: &LlamaBackend,
    k: &Knobs,
) -> Res<(Vec<Step>, Acc)> {
    let cfg = GameConfig { mode: GameMode::Invited, ai_count: k.ai_count };
    let mut s = GameSession::new(&cfg);
    s.rounds_total = k.rounds; // bench-local override (BENCH_ROUNDS, not UNINVITED_ROUNDS)

    let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(k.n_ctx));
    let mut ctx = model.new_context(backend, params)?;
    let mut acc = Acc::default();
    let mut steps: Vec<Step> = Vec::new();

    // One generation under the "current" strategy: clear the whole KV cache,
    // prefill the full prompt, generate. Records the step.
    let mut run = |ctx: &mut LlamaContext,
                   acc: &mut Acc,
                   player: u8,
                   system: String,
                   user: String|
     -> Res<String> {
        let tokens = tokenize(model, &build_prompt(model, &system, &user, k.thinking))?;
        if tokens.len() as u32 >= k.n_ctx {
            eprintln!("[bench] prompt exceeds context window; skipping a turn");
            steps.push(Step { player, system, user, gen_len: 0 });
            return Ok(String::new());
        }
        ctx.clear_kv_cache_seq(Some(0), Some(0), None)?; // full clear, as the app does
        let n_past = prefill(ctx, &tokens, 0, acc)?;
        let g = generate(model, ctx, n_past, k.n_ctx, k.max_tokens, None, k.thinking, acc)?;
        steps.push(Step { player, system, user, gen_len: g.produced });
        Ok(g.text)
    };

    let n = s.players.len();
    let total_questions = k.rounds as usize * n;
    let mut asker_id = pick_first_asker(&s);
    let mut prev_asker: Option<u8> = None;

    for _ in 0..total_questions {
        let target_id = pick_target(&s, asker_id, prev_asker);
        let asker = s.player(asker_id).clone();

        // The asker speaks: invited players ask; the outsider runs its guess/ask
        // turn (we keep its text either way so the game never ends early — a full
        // fixed-length game keeps both strategies on the same workload).
        let (sys, user) = match &asker.role {
            Role::Uninvited => agents::uninvited_turn_prompt(&s, &asker, target_id),
            Role::Invited { .. } => agents::ask_prompt(&s, &asker, target_id),
        };
        let raw = run(&mut ctx, &mut acc, asker_id, sys, user)?;
        let question = match &asker.role {
            Role::Uninvited => match agents::parse_uninvited_turn(&raw) {
                UninvitedAction::Guess(g) => g,
                UninvitedAction::Ask(q) => q,
            },
            Role::Invited { .. } => agents::clean_line(&raw),
        };
        let question = if question.trim().is_empty() { "So, are you excited?".to_string() } else { question };

        // The target answers.
        let target = s.player(target_id).clone();
        let (sys, user) = agents::answer_prompt(&s, &target, asker_id, &question);
        let raw = run(&mut ctx, &mut acc, target_id, sys, user)?;
        let answer = {
            let a = agents::clean_line(&raw);
            if a.is_empty() { "…".to_string() } else { a }
        };

        s.transcript.push(Turn { asker: asker_id, target: target_id, question, answer });

        // The answerer becomes the next asker (SpyFall chain).
        prev_asker = Some(asker_id);
        asker_id = target_id;
    }

    // Vote phase: everyone names a suspect.
    let voter_ids: Vec<u8> = s.players.iter().map(|p| p.id).collect();
    for voter_id in voter_ids {
        let voter = s.player(voter_id).clone();
        let (sys, user) = agents::vote_prompt(&s, &voter);
        let raw = run(&mut ctx, &mut acc, voter_id, sys, user)?;
        let target = agents::parse_vote(&s, voter_id, &raw);
        s.players[voter_id as usize].vote_for = Some(target);
    }

    Ok((steps, acc))
}

/// Pass 2: replay the recorded prompts under the "per-agent" strategy. Each
/// player has its own persistent context; we keep the longest common token
/// prefix with that player's previous prompt and prefill only the new suffix,
/// then generate the same number of tokens the step produced in pass 1.
fn replay_per_agent(
    model: &LlamaModel,
    backend: &LlamaBackend,
    k: &Knobs,
    steps: &[Step],
    n_players: usize,
) -> Res<Acc> {
    let params = || LlamaContextParams::default().with_n_ctx(NonZeroU32::new(k.n_ctx));
    let mut ctxs: Vec<LlamaContext> = Vec::with_capacity(n_players);
    for _ in 0..n_players {
        ctxs.push(model.new_context(backend, params())?);
    }
    // Each agent's previously-prefilled prompt tokens, for the prefix match.
    let mut prev: Vec<Vec<LlamaToken>> = vec![Vec::new(); n_players];
    let mut acc = Acc::default();

    for step in steps {
        if step.gen_len == 0 {
            continue; // a turn skipped during recording (over-long prompt)
        }
        let pid = step.player as usize;
        let tokens = tokenize(model, &build_prompt(model, &step.system, &step.user, k.thinking))?;
        if tokens.len() as u32 >= k.n_ctx {
            continue;
        }
        let ctx = &mut ctxs[pid];

        // Reuse the shared prefix; keep at least one token short of the end so
        // the final position is re-decoded and has fresh logits to sample from.
        let keep = common_prefix_len(&prev[pid], &tokens).min(tokens.len() - 1);
        // Drop KV at positions [keep, end): the divergent suffix from last turn
        // *and* last turn's generated tokens (which sit past the prompt length).
        ctx.clear_kv_cache_seq(Some(0), Some(keep as u32), None)?;
        let n_past = prefill(ctx, &tokens, keep, &mut acc)?;
        generate(model, ctx, n_past, k.n_ctx, k.max_tokens, Some(step.gen_len), k.thinking, &mut acc)?;

        prev[pid] = tokens; // next turn matches against this prompt
    }

    Ok(acc)
}

// -- model loading (same cache as the app) ---------------------------------

/// Make sure the brain is on disk in the app's cache; download it once via the
/// normal Engine path if not. Then load it directly so we control the contexts.
fn ensure_and_load(backend: &LlamaBackend) -> Res<LlamaModel> {
    let brain = brains::active();
    if !brain.is_downloaded() {
        eprintln!("[bench] brain not cached; downloading via the app path (one time)…");
        let engine = Engine::new();
        engine.start();
        engine.load(brains::ACTIVE)?;
        loop {
            let st = engine.status();
            match st.phase {
                Phase::Ready => break,
                Phase::Error => {
                    return Err(format!("download failed: {}", st.error.unwrap_or_default()).into())
                }
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        // Drop the engine (and its worker's own model/context) before we load
        // our own copy, so we don't hold two models in memory.
        drop(engine);
    }
    let params = LlamaModelParams::default().with_n_gpu_layers(999);
    Ok(LlamaModel::load_from_file(backend, &brain.main_path(), &params)?)
}

// -- reporting -------------------------------------------------------------

fn tok_per_s(tokens: u64, d: Duration) -> f64 {
    let s = d.as_secs_f64();
    if s > 0.0 { tokens as f64 / s } else { 0.0 }
}

fn row(name: &str, a: &Acc) {
    println!(
        "{name:<11} | {:>6} | {:>10} | {:>9} | {:>8.2} | {:>8.2} | {:>8.2} | {:>8.0}",
        a.generations,
        a.prefill_tokens,
        a.gen_tokens,
        a.prefill_time.as_secs_f64(),
        a.gen_time.as_secs_f64(),
        a.total().as_secs_f64(),
        tok_per_s(a.prefill_tokens, a.prefill_time),
    );
}

fn pct_drop(from: f64, to: f64) -> f64 {
    if from > 0.0 { (from - to) / from * 100.0 } else { 0.0 }
}

fn main() -> Res<()> {
    let k = knobs();
    println!("[bench] scenarios available: {}", scenarios::all_parties().len());
    println!(
        "[bench] config: {} AI + 1 = {} players, {} round(s), max_tokens={}, n_ctx={}, thinking={}",
        k.ai_count,
        k.ai_count as usize + 1,
        k.rounds,
        k.max_tokens,
        k.n_ctx,
        k.thinking,
    );

    let backend = LlamaBackend::init()?;
    let model = ensure_and_load(&backend)?;
    let n_players = k.ai_count as usize + 1;

    println!("[bench] pass 1/2: current strategy (shared context, cleared each turn)…");
    let (steps, current) = play_and_record(&model, &backend, &k)?;

    println!("[bench] pass 2/2: per-agent strategy ({n_players} persistent contexts)…");
    let per_agent = replay_per_agent(&model, &backend, &k, &steps, n_players)?;

    println!();
    println!("strategy    |  gens | prefill_tk |   gen_tk | prefill_s |   gen_s | total_s | prefill_tk/s");
    println!("------------+-------+------------+----------+-----------+---------+---------+-------------");
    row("current", &current);
    row("per-agent", &per_agent);
    println!();
    println!(
        "[bench] per-agent vs current: prefill tokens {:.1}% fewer, prefill time {:.1}% lower, total {:.1}% lower",
        pct_drop(current.prefill_tokens as f64, per_agent.prefill_tokens as f64),
        pct_drop(current.prefill_time.as_secs_f64(), per_agent.prefill_time.as_secs_f64()),
        pct_drop(current.total().as_secs_f64(), per_agent.total().as_secs_f64()),
    );
    println!(
        "[bench] cost of per-agent: {n_players} live KV caches (~{n_players}x the context memory) vs 1."
    );

    // llama.cpp/Metal can hang on normal teardown; exit hard once flushed.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    unsafe { libc::_exit(0) }
}
