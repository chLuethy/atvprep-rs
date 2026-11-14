import { describe, it, expect } from "vitest";
import { render } from "@solidjs/testing-library";
import App from "../src/App";

describe("App", () => {
  it("renders the atvprep-manager title", () => {
    const { getByText } = render(() => <App />);
    expect(getByText("atvprep-manager")).toBeInTheDocument();
  });

  it("renders navigation links for Files, Activity, Overview, Settings", () => {
    const { getByRole } = render(() => <App />);
    const nav = getByRole("navigation");
    expect(nav).toBeInTheDocument();

    expect(getByRole("link", { name: "Files" })).toBeInTheDocument();
    expect(getByRole("link", { name: "Overview" })).toBeInTheDocument();
    expect(getByRole("link", { name: "Activity" })).toBeInTheDocument();
    expect(getByRole("link", { name: "Settings" })).toBeInTheDocument();
  });
});
