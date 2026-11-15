# Coding Standards für den Review

Ermessensregeln, an denen ein Review nachprüft, was Tooling nicht erzwingt. Jede Regel nennt ihre Quelle.

- **Planer-Ergebnisse kommen vom Server.** Kodierbedarf, Änderungsart und geplante Schritte berechnet der Server (Planer in `atvprep-core`). Das Frontend zeigt sie an und sagt sie nie selbst voraus, auch nicht als Vorschau-Schätzung.
  Quelle: `docs/adr/0008-kodierfreigabe-statt-reencode-schalter.md`, `GLOSSARY.md` „Änderungsart“.
- **Fachbegriffe folgen dem Glossar.** Namen im Code, in UI-Texten, Commits und Docs verwenden die Begriffe aus `GLOSSARY.md` (etwa Vorbereiten, nicht Fix oder Convert).
  Quelle: `GLOSSARY.md`.
- **Sprache.** Agententexte (Issues, PRs, Commits, Glossar, ADRs) sind deutsch; Code, Bezeichner, Code-Kommentare und Label-Namen englisch.
  Quelle: `docs/agents/language.md`.
- **UI-Texte laufen über `t()`, `tn()` oder `N()`** und haben einen deutschen Eintrag unter `frontend/src/locales/de*.ts`. Der englische Text ist der Schlüssel.
  Quelle: `frontend/src/lib/i18n.ts`; `frontend/tests/i18n.test.ts` prüft die Vollständigkeit der Schlüssel, nicht ob ein Text überhaupt durch `t()` läuft.
- **Gemerkter Ansichtsstand nur über `viewMemory.ts`.** Filter, Suche, Ordnung, Spaltenpfad und Layout merkt sich das Frontend mit `remember`/`recall`/`forget`, nie mit direktem Zugriff auf `localStorage`.
  Quelle: `frontend/src/lib/viewMemory.ts`.
