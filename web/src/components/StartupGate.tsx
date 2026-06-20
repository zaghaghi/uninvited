import { useRef } from "react";
import type { ServerStatus } from "../types";
import { DUR, EASE, gsap, reduceMotion, useGSAP } from "../anim";

function fmtBytes(n: number): string {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB"];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${u[i]}`;
}

export function StartupGate({ status }: { status: ServerStatus | null }) {
  const phase = status?.phase ?? "connecting";
  const pct = status ? Math.round(status.progress * 100) : 0;

  const heading: Record<string, string> = {
    connecting: "Waking the host…",
    idle: "Awaiting the guest list…",
    downloading: "Inviting the guests…",
    loading: "Setting the table…",
    error: "Something went wrong",
    ready: "Ready",
  };

  const showBar = phase === "downloading";

  // Mount-only entrance (no deps): this re-renders on every progress tick, but
  // we want the panel to ease in just once, not on each update.
  const panel = useRef<HTMLDivElement | null>(null);
  useGSAP(
    () => {
      if (reduceMotion()) return;
      gsap.from(panel.current, { opacity: 0, y: 14, duration: DUR.slow, ease: EASE });
    },
    { scope: panel },
  );

  return (
    <div className="setup">
      <div className="panel startup" ref={panel}>
        <div className="dossier">UNINVITED // PREPARING THE PARTY</div>
        <h2>{heading[phase] ?? "Preparing…"}</h2>

        {showBar && (
          <>
            <div className="bar">
              <div className="bar-fill" style={{ width: `${pct}%` }} />
            </div>
            <div className="bar-meta">
              <span>{pct}%</span>
              <span>
                {fmtBytes(status!.downloadedBytes)} / {fmtBytes(status!.totalBytes)}
              </span>
            </div>
            <p className="startup-note">First run only — the model is cached for next time.</p>
          </>
        )}

        {!showBar && phase !== "error" && (
          <div className="spinner-row">
            <span className="spinner" />
            <span>{status?.message ?? "Waiting for the local model…"}</span>
          </div>
        )}

        {phase === "error" && (
          <div className="error" style={{ marginTop: 12 }}>
            {status?.error ?? "The model could not be prepared."}
          </div>
        )}
      </div>
    </div>
  );
}
