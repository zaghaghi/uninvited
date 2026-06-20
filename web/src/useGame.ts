import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { AgentActivity, GameView, ServerStatus } from "./types";

// Subscribes to the three backend events: "status" (model load), "game-state"
// (the secret-filtered snapshot), and "agent-activity" (one line per AI action,
// used to animate the scene and the optional thoughts panel).
//
// The `cancelled` flag + unlisten-on-resolve avoids the React 18 StrictMode
// double-subscribe race (the effect tears down before the first async `listen`
// resolves), which would otherwise make every event fire twice.
export function useGame() {
  const [status, setStatus] = useState<ServerStatus | null>(null);
  const [game, setGame] = useState<GameView | null>(null);
  const [activities, setActivities] = useState<AgentActivity[]>([]);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];
    (async () => {
      const subscribe = async <T,>(
        event: string,
        handler: (payload: T) => void,
      ) => {
        const un = await listen<T>(event, (e) => handler(e.payload));
        if (cancelled) un();
        else unlisteners.push(un);
      };
      await subscribe<ServerStatus>("status", setStatus);
      await subscribe<GameView>("game-state", setGame);
      await subscribe<AgentActivity>("agent-activity", (a) =>
        setActivities((cur) => [...cur, a]),
      );
      // Seed the current status. The boot sequence's "status" events stop once
      // the model is ready, so if that happened before the listener above was
      // live (common when the model is cached), we'd never hear it. We attach
      // the listener first, then fetch, and only adopt the fetched value if no
      // event has arrived yet (`cur ?? initial`) so a fresher event is never
      // clobbered by a stale in-flight fetch.
      try {
        const initial = await invoke<ServerStatus>("get_status");
        if (!cancelled) setStatus((cur) => cur ?? initial);
      } catch {
        // The event listener remains the source of truth if this fails.
      }
    })();
    return () => {
      cancelled = true;
      for (const un of unlisteners) un();
    };
  }, []);

  // Start of a new game: forget the previous game's snapshot and activity log.
  const resetLocal = useCallback(() => {
    setGame(null);
    setActivities([]);
  }, []);

  return { status, game, activities, resetLocal };
}
