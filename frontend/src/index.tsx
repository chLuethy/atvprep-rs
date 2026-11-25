import { render } from "@solidjs/web";
import App from "./App";
import { ManagerApi } from "./lib/api";
import { AuthGate } from "./features/auth/AuthGate";
import "./styles/app.css";
import { applyTheme, readStoredTheme } from "./ui/ThemeSwitch";

const root = document.getElementById("root");

if (root) {
  try { window.localStorage.removeItem("atvprep-palette"); } catch { /* Veraltete Farbwahl wird nicht mehr verwendet. */ }
  applyTheme(readStoredTheme());
  const api = new ManagerApi();
  render(
    () => (
      <AuthGate api={api}>
        {(session) => <App api={api} session={session} />}
      </AuthGate>
    ),
    root,
  );
}
