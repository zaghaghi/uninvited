// Headless smoke: bypasses the GUI and exercises the text engine — load the
// brain, run one generation, print the reply (and its reasoning if thinking is
// on).
//
//   cargo run --release --example headless -p uninvited
//
// First run downloads ~3.35 GB into ~/.xaghoul-games/brains/.

use std::time::Duration;

use uninvited::brains;
use uninvited::inference::{Engine, Phase};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::new();
    engine.start();
    engine.load(brains::ACTIVE)?;

    loop {
        let s = engine.status();
        println!(
            "[headless] phase={:?} progress={:.2} msg={} bytes={}/{}",
            s.phase, s.progress, s.message, s.downloaded_bytes, s.total_bytes
        );
        match s.phase {
            Phase::Ready => break,
            Phase::Error => {
                return Err(format!("load failed: {}", s.error.unwrap_or_default()).into());
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let reply = engine
        .generate(
            "You are a cheerful guest at a party. Reply in ONE short, natural sentence.",
            "Another guest asks you: so, how do you know the host?",
            1024,
        )
        .await?;
    if let Some(t) = &reply.thinking {
        println!("[headless] thinking: {t}");
    }
    println!("[headless] reply: {}", reply.text);

    use std::io::Write;
    let _ = std::io::stdout().flush();
    unsafe { libc::_exit(0) }
}
