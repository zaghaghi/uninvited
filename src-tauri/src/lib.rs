//! Uninvited: a local-LLM party deduction game. This library holds the
//! in-process llama.cpp text engine, the brain catalog/downloader, the game
//! rules, the agent prompts, and the orchestrator that runs a game over Tauri
//! events. Shared by the Tauri binary and the headless example.

pub mod agents;
pub mod brains;
pub mod commands;
pub mod game;
pub mod inference;
pub mod orchestrator;
pub mod scenarios;

use tauri::{Manager, RunEvent};

use crate::commands::AppState;
use crate::inference::Engine;

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::start_game,
            commands::submit_question,
            commands::submit_answer,
            commands::submit_vote,
            commands::submit_location_guess,
            commands::call_vote,
            commands::get_status,
            commands::all_parties,
            commands::reset_game,
        ])
        .setup(|app| {
            let engine = Engine::new();
            engine.start();
            commands::spawn_boot(engine.clone(), app.handle().clone());
            app.manage(AppState::new(engine));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Uninvited");

    // Skip ggml's Metal atexit destructor (it aborts on a normal exit); _exit
    // reclaims the process cleanly. Everything we need is already on disk.
    app.run(|_app_handle, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            unsafe { libc::_exit(0) };
        }
    });
}
