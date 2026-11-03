/** True while the keyboard is writing into a field, where single letters are text, not commands. */
export const isTyping = (target: EventTarget | null): boolean =>
  target instanceof HTMLElement && (/^(INPUT|SELECT|TEXTAREA)$/.test(target.tagName) || target.isContentEditable);
