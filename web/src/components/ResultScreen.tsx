import { Fragment, useRef } from "react";
import type { GameView } from "../types";
import { DUR, EASE, EASE_POP, gsap, reduceMotion, useGSAP } from "../anim";

export function ResultScreen({
  game,
  onPlayAgain,
}: {
  game: GameView;
  onPlayAgain: () => void;
}) {
  const humanWon = game.humanInvited
    ? game.winner === "invited"
    : game.winner === "uninvited";
  const outsider = game.players.find((p) => p.id === game.uninvitedId);
  const guesser =
    game.guess && game.players.find((p) => p.id === game.guess!.player)?.name;
  const nameOf = (id: number) => game.players.find((p) => p.id === id)?.name ?? "—";
  // The invited human personally fingered the outsider, but the table didn't
  // rally behind them — so the outsider escaped despite the player being right.
  const humanVote = game.votes?.find((v) => v.voter === game.humanId)?.target;
  const playerWasRight =
    game.humanInvited &&
    game.winner === "uninvited" &&
    humanVote === game.uninvitedId;
  // Tint the outsider and the human so the recap reads at a glance.
  const tint = (id: number | null) =>
    id == null
      ? ""
      : id === game.uninvitedId
        ? "is-uninvited"
        : id === game.humanId
          ? "is-you"
          : "";

  // Stage the reveal: the verdict punches in, then the recap lines and the
  // who-voted-for-whom grid deal in beneath it. The win verdict gets an extra
  // celebratory wobble; the loss just lands.
  const panel = useRef<HTMLDivElement | null>(null);
  useGSAP(
    () => {
      if (reduceMotion()) return;
      const tl = gsap.timeline();
      tl.from(panel.current, { opacity: 0, y: 18, duration: DUR.base, ease: EASE })
        .from(
          ".verdict",
          { opacity: 0, scale: 0.6, duration: DUR.slow, ease: EASE_POP },
          "-=0.1",
        )
        .from(
          ".result .result-line, .result .muted",
          { opacity: 0, y: 10, duration: DUR.base, ease: EASE, stagger: 0.08 },
          "-=0.25",
        )
        .from(
          ".votes-recap-grid > *",
          { opacity: 0, y: 8, duration: DUR.fast, ease: EASE, stagger: 0.03 },
          "-=0.15",
        );
      if (humanWon) {
        tl.from(
          ".verdict",
          { rotation: -4, duration: 0.5, ease: "elastic.out(1, 0.4)" },
          "<",
        );
      }
    },
    { scope: panel },
  );

  return (
    <div className="setup">
      <div className="panel result" ref={panel}>
        <div className={`verdict ${humanWon ? "win" : "lose"}`}>
          {humanWon ? "You win! 🎉" : "You lose"}
        </div>

        <p className="result-line">
          The party was <strong className="party-occ">{game.occasion ?? "—"}</strong>
          {game.location && <span className="party-loc"> — {game.location}</span>}.
        </p>
        <p className="result-line">
          The outsider was <strong>{outsider?.name ?? "—"}</strong>.
        </p>

        {game.guess && (
          <p className="muted">
            {guesser} guessed “{game.guess.guess}” — {game.guess.correct ? "correct!" : "wrong."}
          </p>
        )}

        <p className="muted">
          {game.winner === "invited"
            ? "The guests caught the gatecrasher."
            : playerWasRight
              ? "You spotted the outsider — the others didn't."
              : "The outsider slipped away."}
        </p>

        {game.votes && game.votes.length > 0 && (
          <div className="votes-recap">
            <div className="votes-recap-title">Who voted for whom</div>
            <div className="votes-recap-grid">
              {game.votes.map((v) => (
                <Fragment key={v.voter}>
                  <span className={`vr-name vr-voter ${tint(v.voter)}`}>{nameOf(v.voter)}</span>
                  <span className="vr-arrow" aria-hidden="true">→</span>
                  <span className={`vr-name vr-target ${tint(v.target)}`}>
                    {v.target != null ? nameOf(v.target) : "—"}
                  </span>
                </Fragment>
              ))}
            </div>
          </div>
        )}

        <button className="primary big" onClick={onPlayAgain}>
          Play again
        </button>
      </div>
    </div>
  );
}
