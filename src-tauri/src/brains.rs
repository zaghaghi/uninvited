// The "brain" catalog: the QAT GGUF build of gemma-4-E2B (text only — this game
// needs no vision), downloaded from HuggingFace on demand. A single file: the
// quantized text LM.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Brain {
    pub id: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    #[serde(skip)]
    pub repo: &'static str,
    pub main_file: &'static str,
    pub main_size_bytes: u64,
}

/// The only brain, auto-loaded on startup.
pub const ACTIVE: &str = "gemma-4-e2b";

pub const CATALOG: &[Brain] = &[Brain {
    id: "gemma-4-e2b",
    label: "Gemma 4 E2B QAT",
    blurb: "A small, sharp Gemma 4. Downloads the QAT text model on first run.",
    repo: "google/gemma-4-E2B-it-qat-q4_0-gguf",
    main_file: "gemma-4-E2B_q4_0-it.gguf",
    main_size_bytes: 3_350_000_000,
}];

pub fn find(id: &str) -> Option<&'static Brain> {
    CATALOG.iter().find(|b| b.id == id)
}

pub fn active() -> &'static Brain {
    find(ACTIVE).expect("ACTIVE brain missing from CATALOG")
}

/// Where downloaded brains are cached (persists across runs).
pub fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".xaghoul-games")
        .join("brains")
}

impl Brain {
    pub fn main_path(&self) -> PathBuf {
        cache_dir().join(self.main_file)
    }
    pub fn is_downloaded(&self) -> bool {
        self.main_path().exists()
    }
    pub fn resolve_url(&self, file: &str) -> String {
        format!("https://huggingface.co/{}/resolve/main/{}", self.repo, file)
    }
}
