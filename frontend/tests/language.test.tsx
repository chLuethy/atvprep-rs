import { fireEvent, render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "../src/App";
import { setLanguage } from "../src/lib/i18n";
import { formatSize } from "../src/features/files/model";

afterEach(() => {
  setLanguage("en");
  window.localStorage.clear();
});

describe("the language switch in the shell", () => {
  it("turns the navigation German at once and back again", async () => {
    const view = render(() => <App />);
    expect(view.getByRole("navigation", { name: "Main" })).toBeInTheDocument();

    await fireEvent.click(view.getByRole("button", { name: "Deutsch" }));
    expect(view.getByRole("navigation", { name: "Hauptnavigation" })).toBeInTheDocument();
    expect(view.getByRole("link", { name: "Mediathek" })).toBeInTheDocument();
    expect(view.getByRole("link", { name: "Einstellungen" })).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("de");

    await fireEvent.click(view.getByRole("button", { name: "English" }));
    expect(view.getByRole("link", { name: "Library" })).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("en");
  });

  it("formats numbers the way the language does", () => {
    expect(formatSize(1536)).toBe("1.5 KB");
    setLanguage("de");
    expect(formatSize(1536)).toBe("1,5 KB");
  });
});
