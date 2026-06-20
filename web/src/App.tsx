import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useGame } from "./useGame";
import { StartupGate } from "./components/StartupGate";
import { ConfigScreen } from "./components/ConfigScreen";
import { GameScreen } from "./components/GameView";
import { ResultScreen } from "./components/ResultScreen";
import type { GameConfig } from "./types";

export default function App() {
  const { status, game, activities, resetLocal } = useGame();

  const startGame = useCallback(
    async (config: GameConfig) => {
      resetLocal();
      await invoke("start_game", { config });
    },
    [resetLocal],
  );

  const playAgain = useCallback(async () => {
    await invoke("reset_game");
    resetLocal();
  }, [resetLocal]);

  let body;
  if (!status || status.phase !== "ready") {
    body = <StartupGate status={status} />;
  } else if (!game) {
    body = <ConfigScreen onStart={startGame} />;
  } else if (game.phase === "reveal") {
    body = <ResultScreen game={game} onPlayAgain={playAgain} />;
  } else {
    body = <GameScreen game={game} activities={activities} />;
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          UNINVITED <small>// everyone's invited. except one.</small>
        </div>
      </header>
      {body}
    </div>
  );
}
