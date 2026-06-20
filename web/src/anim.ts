// One shared motion vocabulary for the whole UI. Components import gsap/useGSAP
// from here (not the packages directly) so the plugin is registered exactly once
// and every entrance speaks the same easing/duration language.
//
// Design rule: animate with `gsap.from()` only. The natural DOM state is always
// the correct, fully-visible one — so if a tween never runs (reduced motion, or
// a future change) the screen is still right. Nothing is hidden by CSS waiting
// to be revealed.
import { gsap } from "gsap";
import { useGSAP } from "@gsap/react";

gsap.registerPlugin(useGSAP);

// Durations (seconds) and easings, named so intent reads at the call site.
export const DUR = { fast: 0.25, base: 0.4, slow: 0.6 } as const;
export const EASE = "power3.out";
export const EASE_POP = "back.out(1.7)";

// Respect the OS "reduce motion" setting: every useGSAP callback bails out when
// this is true, leaving elements at their natural (visible) state.
export function reduceMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true
  );
}

export { gsap, useGSAP };
