// In-process text inference via llama.cpp (the llama-cpp-2 crate). A "brain" —
// the QAT gemma-4-E2B text model — is downloaded from HuggingFace on demand,
// then loaded. All llama.cpp objects live on one dedicated worker thread (they
// aren't Sync); the async orchestrator talks to it over a channel with oneshot
// replies.
//
// Each generation is a fresh prompt: (system, user) -> reply. The prompt is
// tokenized and prefilled from n_past=0 every call (no cross-turn KV reuse — the
// game rebuilds the full public transcript into the `user` string each turn, so
// every agent acts on the same shared history).

use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::json;
use tokio::sync::oneshot;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use minijinja::value::ValueKind;
use minijinja::{Environment, Error, ErrorKind, Value};

use crate::brains::{self, Brain};

const N_CTX: u32 = 8192;
/// Tokens decoded per prefill batch. Kept at/below llama.cpp's default n_batch
/// so a long transcript prompt can be fed in slices without a custom n_batch.
const PREFILL_CHUNK: usize = 512;

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Downloading,
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub model_ready: bool,
    pub model_name: String,
    pub progress: f32,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Status {
    fn idle() -> Self {
        Status {
            phase: Phase::Idle,
            model_ready: false,
            model_name: String::new(),
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            message: "No brain loaded".to_string(),
            error: None,
        }
    }

    /// An orchestrator-originated error status (e.g. load could not be started).
    pub fn error(message: String) -> Self {
        Status {
            phase: Phase::Error,
            model_ready: false,
            model_name: String::new(),
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            message: message.clone(),
            error: Some(message),
        }
    }
}

enum Msg {
    Load {
        brain_id: String,
    },
    Generate {
        system: String,
        user: String,
        max_tokens: usize,
        resp: oneshot::Sender<Result<Reply, String>>,
    },
}

#[derive(Clone)]
pub struct Engine {
    status: Arc<Mutex<Status>>,
    tx: Arc<Mutex<Option<Sender<Msg>>>>,
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            status: Arc::new(Mutex::new(Status::idle())),
            tx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn is_ready(&self) -> bool {
        self.status.lock().unwrap().phase == Phase::Ready
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        let mut s = self.status.lock().unwrap();
        f(&mut s);
        s.model_ready = s.phase == Phase::Ready;
    }

    /// Spawn the worker thread. It stays idle until a brain is requested.
    pub fn start(&self) {
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        *self.tx.lock().unwrap() = Some(tx);
        let me = self.clone();
        std::thread::Builder::new()
            .name("llama-worker".into())
            .spawn(move || me.worker(rx))
            .expect("failed to spawn llama worker");
    }

    /// Request that a brain be downloaded (if needed) and loaded.
    pub fn load(&self, brain_id: &str) -> Result<(), String> {
        let guard = self.tx.lock().unwrap();
        let tx = guard.as_ref().ok_or("engine not started")?;
        tx.send(Msg::Load {
            brain_id: brain_id.to_string(),
        })
        .map_err(|_| "worker gone".to_string())
    }

    /// One text turn: (system, user) -> reply (answer + optional reasoning).
    pub async fn generate(
        &self,
        system: &str,
        user: &str,
        max_tokens: usize,
    ) -> Result<Reply, String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        {
            let guard = self.tx.lock().unwrap();
            let tx = guard.as_ref().ok_or_else(|| "model not ready".to_string())?;
            tx.send(Msg::Generate {
                system: system.to_string(),
                user: user.to_string(),
                max_tokens,
                resp: resp_tx,
            })
            .map_err(|_| "worker gone".to_string())?;
        }
        resp_rx
            .await
            .map_err(|_| "worker dropped response".to_string())?
    }

    // -- worker thread -----------------------------------------------------

    fn worker(&self, rx: Receiver<Msg>) {
        let backend = match LlamaBackend::init() {
            Ok(b) => b,
            Err(e) => {
                self.set(|s| {
                    s.phase = Phase::Error;
                    s.message = "Backend init failed".into();
                    s.error = Some(e.to_string());
                });
                return;
            }
        };

        let mut pending: Option<String> = None;
        loop {
            let brain_id = match pending.take() {
                Some(id) => id,
                None => match self.recv_until_load(&rx) {
                    Some(id) => id,
                    None => return, // channel closed
                },
            };

            let brain = match brains::find(&brain_id) {
                Some(b) => b,
                None => {
                    self.set(|s| {
                        s.phase = Phase::Error;
                        s.message = "Unknown brain".into();
                        s.error = Some(format!("no brain '{brain_id}'"));
                    });
                    continue;
                }
            };

            let model = match self.download_and_load(&backend, brain) {
                Ok(m) => m,
                Err(e) => {
                    self.set(|s| {
                        s.phase = Phase::Error;
                        s.message = "Failed to load brain".into();
                        s.error = Some(e);
                    });
                    continue;
                }
            };

            let ctx_params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(N_CTX));
            let mut ctx = match model.new_context(&backend, ctx_params) {
                Ok(c) => c,
                Err(e) => {
                    self.set(|s| {
                        s.phase = Phase::Error;
                        s.message = "Context init failed".into();
                        s.error = Some(e.to_string());
                    });
                    continue;
                }
            };

            self.set(|s| {
                s.phase = Phase::Ready;
                s.message = format!("{} ready", brain.label);
                s.progress = 1.0;
                s.error = None;
            });

            let thinking = thinking_enabled();
            eprintln!(
                "[uninvited] thinking mode is {} (set UNINVITED_THINKING=false to disable)",
                if thinking { "ON" } else { "OFF" }
            );

            // Serve turns until a reload is requested or the channel closes.
            loop {
                match rx.recv() {
                    Ok(Msg::Generate {
                        system,
                        user,
                        max_tokens,
                        resp,
                    }) => {
                        let r = generate_text(
                            &model, &mut ctx, &system, &user, max_tokens, thinking,
                        );
                        let _ = resp.send(r);
                    }
                    Ok(Msg::Load { brain_id }) => {
                        pending = Some(brain_id);
                        break; // drop model+ctx, reload in the outer loop
                    }
                    Err(_) => return,
                }
            }
        }
    }

    /// Wait for a Load message, answering any stray Generate with an error.
    fn recv_until_load(&self, rx: &Receiver<Msg>) -> Option<String> {
        loop {
            match rx.recv() {
                Ok(Msg::Load { brain_id }) => return Some(brain_id),
                Ok(Msg::Generate { resp, .. }) => {
                    let _ = resp.send(Err("no brain loaded".to_string()));
                }
                Err(_) => return None,
            }
        }
    }

    fn download_and_load(
        &self,
        backend: &LlamaBackend,
        brain: &Brain,
    ) -> Result<LlamaModel, String> {
        if !brain.main_path().exists() {
            self.download_file(brain, brain.main_file, brain.main_size_bytes)?;
        }

        self.set(|s| {
            s.phase = Phase::Loading;
            s.model_name = brain.label.to_string();
            s.message = "Setting the table…".into();
            s.progress = 1.0;
        });

        let params = LlamaModelParams::default().with_n_gpu_layers(999);
        LlamaModel::load_from_file(backend, &brain.main_path(), &params).map_err(|e| e.to_string())
    }

    fn download_file(&self, brain: &Brain, file: &str, size_bytes: u64) -> Result<(), String> {
        std::fs::create_dir_all(brains::cache_dir()).map_err(|e| e.to_string())?;
        self.set(|s| {
            s.phase = Phase::Downloading;
            s.model_name = brain.label.to_string();
            s.message = format!("Downloading {file}…");
            s.progress = 0.0;
            s.downloaded_bytes = 0;
            s.total_bytes = size_bytes;
        });

        let url = brain.resolve_url(file);
        let client = reqwest::blocking::Client::builder()
            .timeout(None)
            .build()
            .map_err(|e| e.to_string())?;
        let mut resp = client
            .get(&url)
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let total = resp.content_length().unwrap_or(size_bytes);

        let final_path = brains::cache_dir().join(file);
        let tmp = final_path.with_extension("part");
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 20];
        let mut downloaded: u64 = 0;
        loop {
            let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            downloaded += n as u64;
            self.set(|s| {
                s.downloaded_bytes = downloaded;
                s.total_bytes = total;
                s.progress = if total > 0 {
                    downloaded as f32 / total as f32
                } else {
                    0.0
                };
            });
        }
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&tmp, &final_path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Whether "thinking" mode is on. **On by default** — the engine renders the
/// model's baked chat template with `enable_thinking=true` so the model reasons
/// before answering. Set `UNINVITED_THINKING=false` (or `0`) to opt out and use
/// the manual Gemma prompt with no reasoning.
pub fn thinking_enabled() -> bool {
    !matches!(
        std::env::var("UNINVITED_THINKING").as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("False")
    )
}

fn debug_raw() -> bool {
    std::env::var("UNINVITED_DEBUG_RAW").is_ok()
}

/// A generated reply: the final answer, plus the reasoning that produced it
/// (present only when thinking mode was actually used this turn).
#[derive(Clone, Debug)]
pub struct Reply {
    pub text: String,
    pub thinking: Option<String>,
}

/// Generate a text reply. When `enable_thinking` is true the prompt is rendered
/// from the model's baked chat template with `enable_thinking=true` (the model
/// then reasons in a thought channel before answering); otherwise the manual
/// Gemma prompt is used. If the template render fails, we fall back to the
/// manual prompt so the app keeps working.
fn generate_text(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    system: &str,
    user: &str,
    max_tokens: usize,
    enable_thinking: bool,
) -> Result<Reply, String> {
    let manual = || build_gemma_prompt(Some(system), &[("user".to_string(), user.to_string())]);

    let (prompt, thinking) = if enable_thinking {
        match render_thinking_template(model, system, user) {
            Ok(p) => (p, true),
            Err(e) => {
                eprintln!("[uninvited] thinking template failed ({e}); falling back");
                (manual(), false)
            }
        }
    } else {
        (manual(), false)
    };

    if debug_raw() {
        eprintln!("[uninvited] PROMPT >>>\n{prompt}\n<<< (thinking={thinking})");
    }

    let tokens = model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|e| e.to_string())?;
    if tokens.len() as u32 >= N_CTX {
        return Err("prompt longer than context window".to_string());
    }

    // Prefill from a clean KV cache, in chunks of PREFILL_CHUNK tokens so long
    // transcripts fit within llama.cpp's batch size. Only the final token needs
    // logits (that's where generation starts).
    ctx.clear_kv_cache_seq(Some(0), Some(0), None)
        .map_err(|e| e.to_string())?;
    let n_prompt = tokens.len();
    let mut batch = LlamaBatch::new(PREFILL_CHUNK, 1);
    let mut i = 0usize;
    while i < n_prompt {
        let end = (i + PREFILL_CHUNK).min(n_prompt);
        batch.clear();
        for j in i..end {
            let is_last = j == n_prompt - 1;
            batch
                .add(tokens[j], j as i32, &[0], is_last)
                .map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;
        i = end;
    }

    let mut n_past = n_prompt as i32;
    let max_total = (n_past + max_tokens as i32).min(N_CTX as i32 - 1);
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::temp(0.8),
        LlamaSampler::dist(seed()),
    ]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut out = String::new();

    while n_past < max_total {
        // idx -1 = the last position's logits (the last decoded token).
        let token = sampler.sample(ctx, -1);
        sampler.accept(token);
        if model.is_eog_token(token) {
            break;
        }
        // `special` renders special tokens as their text piece. On the thinking
        // path the model emits thought/answer delimiters as special tokens
        // (`<|channel>thought`, `<channel|>`, `<|channel>final`); they MUST be
        // rendered so `split_thinking` can find them. On the manual path strip
        // them (the EOG token is already handled above).
        let piece = model
            .token_to_piece(token, &mut decoder, thinking, None)
            .map_err(|e| e.to_string())?;
        out.push_str(&piece);
        if let Some(idx) = out
            .find("<end_of_turn>")
            .or_else(|| out.find("<start_of_turn>"))
        {
            out.truncate(idx);
            break;
        }

        batch.clear();
        batch
            .add(token, n_past, &[0], true)
            .map_err(|e| e.to_string())?;
        n_past += 1;
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;
    }

    if debug_raw() {
        eprintln!("[uninvited] RAW OUTPUT >>>\n{out}\n<<<");
    }

    Ok(split_thinking(out, thinking))
}

/// Render the model's baked chat template with `enable_thinking=true`. The
/// system text is folded into the single user message (Gemma has no system
/// role), matching the manual path. `pub` so the KV-cache benchmark example
/// (examples/bench_kv.rs) can build prompts identical to production.
pub fn render_thinking_template(
    model: &LlamaModel,
    system: &str,
    instruction: &str,
) -> Result<String, String> {
    let tmpl_str = model
        .chat_template(None)
        .map_err(|e| format!("get chat template: {e}"))?
        .to_string()
        .map_err(|e| format!("chat template utf8: {e}"))?;

    let mut env = Environment::new();
    // The official Gemma 4 chat template calls Python dict methods like
    // `message.get('reasoning')`; minijinja doesn't expose `.get()` on plain
    // maps, which silently breaks thinking. Teach minijinja `.get` via the
    // unknown-method callback so the real template renders.
    env.set_unknown_method_callback(|_state, value, method, args| {
        if method == "get" && value.kind() == ValueKind::Map {
            let key = match args.first() {
                Some(k) => k,
                None => return Err(Error::from(ErrorKind::InvalidOperation)),
            };
            let v = value.get_item(key)?;
            if matches!(v.kind(), ValueKind::Undefined | ValueKind::None) {
                Ok(args.get(1).cloned().unwrap_or(Value::UNDEFINED))
            } else {
                Ok(v)
            }
        } else {
            Err(Error::from(ErrorKind::UnknownMethod))
        }
    });
    let tmpl = env
        .template_from_str(&tmpl_str)
        .map_err(|e| format!("jinja parse: {e}"))?;

    let content = if system.trim().is_empty() {
        instruction.to_string()
    } else {
        format!("{}\n\n{}", system.trim(), instruction)
    };
    let ctx = json!({
        "messages": [{ "role": "user", "content": content }],
        "add_generation_prompt": true,
        "enable_thinking": true,
        "tools": [],
        "date_string": "",
    });
    tmpl.render(&ctx).map_err(|e| format!("jinja render: {e}"))
}

/// Split a thinking-mode output into (answer, reasoning). Gemma 4 emits
/// reasoning in a thought channel, then the answer. Token spellings vary across
/// templates/quantizations, so we match loosely. Returns the answer plus the
/// reasoning text (None when thinking was off or no thought channel appeared).
pub fn split_thinking(raw: String, thinking: bool) -> Reply {
    if !thinking {
        return Reply {
            text: raw.trim().to_string(),
            thinking: None,
        };
    }
    let lower: String = raw.to_ascii_lowercase();

    let find_open = |s: &str, label: &str| -> Option<usize> {
        s.find(&format!("<|channel|{label}"))
            .or_else(|| s.find(&format!("<|channel>{label}")))
    };
    let close_after = |from: usize| -> Option<usize> {
        let after = &lower[from..];
        after
            .find("<channel|>")
            .or_else(|| after.find("<|channel|>"))
            .map(|c| from + c)
    };
    let close_len_at = |pos: usize| -> usize {
        if raw[pos..].starts_with("<channel|>") {
            10
        } else {
            11
        }
    };
    let content_after = |open: usize| -> usize {
        match raw[open..].find('\n') {
            Some(n) => open + n + 1,
            None => open,
        }
    };

    let thought_open = find_open(&lower, "thought");
    let final_open = lower
        .rfind("<|channel|final")
        .or_else(|| lower.rfind("<|channel>final"));

    let (answer_raw, reasoning) = match (thought_open, final_open) {
        (Some(to), Some(fo)) if to < fo => {
            let cstart = content_after(to);
            let reasoning = close_after(cstart).map(|c| raw[cstart..c].trim().to_string());
            let astart = content_after(fo);
            let answer = match close_after(astart) {
                Some(c) => raw[astart..c].to_string(),
                None => raw[astart..].to_string(),
            };
            (answer, reasoning)
        }
        (Some(to), None) => {
            let cstart = content_after(to);
            let (reasoning, answer) = match close_after(cstart) {
                Some(c) => (
                    Some(raw[cstart..c].trim().to_string()),
                    raw[c + close_len_at(c)..].to_string(),
                ),
                None => (Some(raw[cstart..].trim().to_string()), String::new()),
            };
            (answer, reasoning)
        }
        (None, Some(fo)) => {
            let astart = content_after(fo);
            let answer = match close_after(astart) {
                Some(c) => raw[astart..c].to_string(),
                None => raw[astart..].to_string(),
            };
            (answer, None)
        }
        (None, None) => (raw, None),
        _ => (raw, None),
    };

    let answer = answer_raw
        .replace("<channel|>", "")
        .replace("<|channel|>", "")
        .trim()
        .to_string();
    let reasoning = reasoning
        .map(|r| {
            r.replace("<channel|>", "")
                .replace("<|channel|>", "")
                .trim()
                .to_string()
        })
        .filter(|r| !r.is_empty());
    Reply {
        text: answer,
        thinking: reasoning,
    }
}

fn seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

/// Build a Gemma chat-format prompt. Gemma has no system role, so the system
/// text is folded into the first user turn. BOS is added by the tokenizer.
pub fn build_gemma_prompt(system: Option<&str>, messages: &[(String, String)]) -> String {
    let mut out = String::new();
    let sys = system.unwrap_or("").trim().to_string();
    let mut sys_used = false;
    for (role, content) in messages {
        if role == "assistant" {
            out.push_str("<start_of_turn>model\n");
            out.push_str(content.trim());
            out.push_str("<end_of_turn>\n");
        } else {
            out.push_str("<start_of_turn>user\n");
            if !sys.is_empty() && !sys_used {
                out.push_str(&sys);
                out.push_str("\n\n");
                sys_used = true;
            }
            out.push_str(content.trim());
            out.push_str("<end_of_turn>\n");
        }
    }
    if !sys.is_empty() && !sys_used {
        out.push_str("<start_of_turn>user\n");
        out.push_str(&sys);
        out.push_str("<end_of_turn>\n");
    }
    out.push_str("<start_of_turn>model\n");
    out
}
