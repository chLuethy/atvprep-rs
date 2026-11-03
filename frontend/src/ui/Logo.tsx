import type { Component } from "solid-js";
import { WORDMARK_PATH } from "./brandPaths";

/** The tile: a play triangle that was sewn up. Lime in both themes, so it reads on any surface. */
export const LogoMark: Component<{ size?: number }> = (props) => (
  <svg class="logo-mark" width={props.size ?? 24} height={props.size ?? 24} viewBox="0 0 64 64" aria-hidden="true">
    <rect width="64" height="64" rx="15" fill="var(--brand-lime)" />
    <g transform="translate(-2 0)">
      <path d="M17 12L55 32 17 52z" fill="var(--brand-ink)" stroke="var(--brand-ink)" stroke-width="5" stroke-linejoin="round" />
      <path d="M31 4v56" stroke="var(--brand-lime)" stroke-width="4" />
      <path d="M26 25h10M26 32h10M26 39h10" stroke="var(--brand-ink)" stroke-width="3.2" stroke-linecap="round" />
    </g>
  </svg>
);

/**
 * Tile and name, with the cursor after it. The name is outlines in the text colour; the cursor is
 * lit while the worker has something in hand and dimmed while it has not.
 */
export const Logo: Component<{ height?: number; busy?: boolean }> = (props) => {
  const height = () => props.height ?? 28;
  return (
    <svg class="logo" classList={{ busy: !!props.busy }} height={height()} width={(height() * 271) / 64} viewBox="0 0 271 64" aria-hidden="true">
      <rect width="64" height="64" rx="15" fill="var(--brand-lime)" />
      <g transform="translate(-2 0)">
        <path d="M17 12L55 32 17 52z" fill="var(--brand-ink)" stroke="var(--brand-ink)" stroke-width="5" stroke-linejoin="round" />
        <path d="M31 4v56" stroke="var(--brand-lime)" stroke-width="4" />
        <path d="M26 25h10M26 32h10M26 39h10" stroke="var(--brand-ink)" stroke-width="3.2" stroke-linecap="round" />
      </g>
      <g transform="translate(76 43.8) scale(0.043 0.043)" fill="currentColor"><path d={WORDMARK_PATH} /></g>
      <rect class="logo-cursor" x="247.57" y="12.41" width="21.5" height="40" rx="1.6" fill="var(--brand-lime)" />
    </svg>
  );
};
