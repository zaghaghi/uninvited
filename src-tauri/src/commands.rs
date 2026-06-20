// The only module the webview talks to. Holds the shared app state (the engine
// plus the channel into the running game), boots the model on startup, and
// exposes the Tauri commands the UI invokes. Backend-driven updates flow the
// other way as "status" / "game-state" / "agent-activity" events.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::game::GameConfig;
use crate::inference::{self, Engine, Phase};
use crate::orchestrator::{self, HumanInput};
use crate::scenarios;

pub struct AppState {
    pub engine: Engine,
    /// Sender into the current game's input channel; `None` between games.
    pub input_tx: Mutex<Option<UnboundedSender<HumanInput>>>,
}

impl AppState {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            input_tx: Mutex::new(None),
        }
    }

    fn send(&self, input: HumanInput) -> Result<(), String> {
        let guard = self.input_tx.lock().unwrap();
        match guard.as_ref() {
            Some(tx) => tx.send(input).map_err(|_| "the game has ended".to_string()),
            None => Err("no game in progress".to_string()),
        }
    }
}

/// Load the model and pump "status" events to the UI until it is ready.
pub fn spawn_boot(engine: Engine, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = engine.load(crate::brains::ACTIVE) {
            let _ = app.emit(
                "status",
                inference::Status::error(format!("could not start the engine: {e}")),
            );
            return;
        }
        let mut last_phase = None;
        let mut last_progress = -1.0f32;
        loop {
            let st = engine.status();
            let changed = last_phase != Some(st.phase)
                || (st.phase == Phase::Downloading && (st.progress - last_progress).abs() >= 0.01);
            if changed {
                last_phase = Some(st.phase);
                last_progress = st.progress;
                let _ = app.emit("status", st.clone());
            }
            if st.phase == Phase::Ready || st.phase == Phase::Error {
                break;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    });
}

#[tauri::command]
pub fn start_game(
    app: AppHandle,
    state: State<AppState>,
    config: GameConfig,
) -> Result<(), String> {
    if !state.engine.is_ready() {
        return Err("the model is still loading".into());
    }
    let (tx, rx) = mpsc::unbounded_channel();
    *state.input_tx.lock().unwrap() = Some(tx);
    let engine = state.engine.clone();
    tauri::async_runtime::spawn(orchestrator::run_game(engine, app, rx, config));
    Ok(())
}

#[tauri::command]
pub fn submit_question(state: State<AppState>, target: u8, text: String) -> Result<(), String> {
    state.send(HumanInput::Question { target, text })
}

#[tauri::command]
pub fn submit_answer(state: State<AppState>, text: String) -> Result<(), String> {
    state.send(HumanInput::Answer { text })
}

#[tauri::command]
pub fn submit_vote(state: State<AppState>, target: u8) -> Result<(), String> {
    state.send(HumanInput::Vote { target })
}

#[tauri::command]
pub fn submit_location_guess(state: State<AppState>, text: String) -> Result<(), String> {
    state.send(HumanInput::GuessLocation { text })
}

/// Bartender mode: end the eavesdropping early and jump straight to the vote.
#[tauri::command]
pub fn call_vote(state: State<AppState>) -> Result<(), String> {
    state.send(HumanInput::CallVote)
}

/// The engine's current status. The boot sequence emits "status" events, but
/// they are fire-and-forget and stop once the model is ready — a webview that
/// attaches its listener after that final event (likely when the model is
/// cached and loads fast) would otherwise never learn it's ready and sit on the
/// startup screen forever. The UI queries this once on mount to seed its state,
/// closing that race.
#[tauri::command]
pub fn get_status(state: State<AppState>) -> inference::Status {
    state.engine.status()
}

/// The public board of every possible party (occasion + venue). Constant for a
/// build, so the UI fetches it once per game rather than receiving it in each
/// snapshot.
#[tauri::command]
pub fn all_parties() -> Vec<scenarios::Party> {
    scenarios::all_parties()
}

/// Abandon the current game (closes the input channel, which ends the task).
#[tauri::command]
pub fn reset_game(state: State<AppState>) -> Result<(), String> {
    *state.input_tx.lock().unwrap() = None;
    Ok(())
}
