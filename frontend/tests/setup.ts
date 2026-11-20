import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";

// What one test leaves in the browser storage (a view choice, a remembered state) must not reach the next.
afterEach(() => {
  try { window.localStorage.clear(); } catch { /* a test replaced the storage */ }
});
