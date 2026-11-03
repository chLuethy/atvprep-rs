import { render } from "solid-js/web";
import App from "./App";
import { ManagerApi } from "./lib/api";
import { AuthGate } from "./features/auth/AuthGate";
import "./styles/app.css";

const root = document.getElementById("root");

if (root) {
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
