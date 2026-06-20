// Uninvited desktop shell. Inference runs in-process via llama-cpp-2
// (Metal GPU, text only). On launch the single brain — the QAT gemma-4-E2B text
// model — is downloaded (first run) and loaded; the UI then drives games over
// Tauri commands while the orchestrator pushes state back over events.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    uninvited::run();
}
