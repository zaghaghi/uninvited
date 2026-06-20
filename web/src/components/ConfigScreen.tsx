import { useRef, useState } from "react";
import type { GameConfig, GameMode } from "../types";
import { DUR, EASE, gsap, reduceMotion, useGSAP } from "../anim";

const MODE_HINTS: Record<GameMode, string> = {
  invited: "You'll know the party. Find the guest who doesn't.",
  outsider: "You won't know the party. Blend in and figure it out.",
  bartender:
    "You won't join the chat. Listen in at the bar, then call out who wasn't really invited.",
};

export function ConfigScreen({ onStart }: { onStart: (c: GameConfig) => void }) {
  const [mode, setMode] = useState<GameMode>("invited");
  const [aiCount, setAiCount] = useState(4);

  // Deal the setup form in line by line on mount, so the party assembles.
  const panel = useRef<HTMLDivElement | null>(null);
  useGSAP(
    () => {
      if (reduceMotion()) return;
      gsap.from(panel.current, { opacity: 0, y: 16, duration: DUR.base, ease: EASE });
      gsap.from(".panel.config > *", {
        opacity: 0,
        y: 12,
        duration: DUR.base,
        ease: EASE,
        stagger: 0.07,
        delay: 0.08,
      });
    },
    { scope: panel },
  );

  return (
    <div className="setup">
      <div className="panel config" ref={panel}>
        <h2>Set up the party</h2>
        <p className="muted">
          Everyone here is invited and knows the party — except one gatecrasher. Invited guests
          hunt the outsider; the outsider hunts the party.
        </p>

        <div className="field">
          <label>You play as</label>
          <div className="seg">
            <button className={mode === "invited" ? "seg-on" : ""} onClick={() => setMode("invited")}>
              Invited guest
            </button>
            <button className={mode === "outsider" ? "seg-on" : ""} onClick={() => setMode("outsider")}>
              The outsider
            </button>
            <button
              className={mode === "bartender" ? "seg-on" : ""}
              onClick={() => setMode("bartender")}
            >
              Bartender
            </button>
          </div>
          <p className="hint">{MODE_HINTS[mode]}</p>
        </div>

        <div className="field">
          <label>
            AI players: <strong>{aiCount}</strong>
          </label>
          <input
            type="range"
            min={2}
            max={8}
            value={aiCount}
            onChange={(e) => setAiCount(Number(e.target.value))}
          />
          <p className="hint">{aiCount + 1} players total. More guests means longer rounds.</p>
        </div>

        <button className="primary big" onClick={() => onStart({ mode, aiCount })}>
          Start the party
        </button>
      </div>
    </div>
  );
}
