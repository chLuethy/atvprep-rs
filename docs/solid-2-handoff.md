# Übergabe: Solid-2-Migration #107–#119

**Fortgesetzt am 20. November 2026; aktueller Prüfstand 13:12 CET.** Die Umsetzung ist weiterhin nicht vollständig abgenommen. **Abschnitt 0c und die [Kriterienmatrix](solid-2-acceptance.md) sind der maßgebliche aktuelle Zwischenstand.** Abschnitt 0 bewahrt den Stand von 05:45 CET; Abschnitte 1–7 bewahren die historische Übergabe von 05:08 CET. Bei Widersprüchen gelten die neueren Abschnitte. Alle Prüfungen sind lokale Belege, keine integrierte Abnahme.

## 0c. Fortsetzung: Zeitplan-Lifecycle und langsame Scan-Polls (#112)

- Einstieg bei `926d321` auf `integration/solid-2`; vorhandene sachfremde Dateien unverändert. Weiterhin sequenzielle Arbeit mit Codemode und `implement-spec`, da keine Subagent-Werkzeuge verfügbar sind. Kein unabhängiger Abschlussreview behauptet.
- **Drei neue Fehler zuerst rot reproduziert:** Nach Verlassen löste eine verspätete Zeitplan-Laufantwort noch einen Listen-Read aus. Außerdem startete die Freigabefakten-Kette nach einer verspäteten Zeitplanantwort noch einen Dashboard-Read. Langsame Scan-Polls stapelten mehrere parallele Requests.
- Zeitplanansicht besitzt nun ein einziges Lebensdauersignal (`AbortController` mit Cleanup). Laden, Speichern, Zeilenaktionen und Freigabefakten verarbeiten nach Unmount keine Seitenänderung oder Folgeaktualisierung mehr. `loadEnableFacts` prüft das Signal zwischen den Reads und reicht es für den Dashboard-Read weiter. Der bereits laufende erste Listen-Read wird nicht als physisch abgebrochen ausgegeben; gestartete Servermutationen werden nicht rückgängig gemacht.
- Scan-Polling überspringt Ticks während eines laufenden Reads. Der vorhandene Disposal-Schutz bleibt erhalten. Keine neue Generationenverwaltung oder allgemeine Datenplattform.
- **Zehn zusätzliche Verhaltenstests am vorhandenen UI-/Request-Seam:** Zeitplan-Lauf, Pause, Löschen, Speicherung und Freigabefakten nach Verlassen; langsamer Scan-Poll, verspäteter Scanabschluss sowie Ende einer Scanfolge über mehrere Mediatheken; verspätete Bereinigung mit Erfolg und 409-Konflikt. Die bisher nur angenommenen Scan-/Bereinigungs-Guards sind damit für diese Fälle nachgewiesen.
- Typecheck und **793 Tests in 43 Dateien bestanden**, unverdeckter Development-Lauf ohne Solid-`[CODE]`-Diagnosen (`/tmp/solid-107/lifecycle-final-tests.log`). Produktionsbuild und weiterhin **33 Chromium-Journeys bestanden**, 23,5 s (`lifecycle-e2e.log`). Keine neue Browser-Journey für diese Antwortreihenfolgen behauptet; sie werden deterministisch in UI-Integrationstests geprüft.
- Vollständige Projektprüfung vor dem zugehörigen #112-Commit: Format, Clippy und **944 Rust-Tests in 71 Testprogrammen bestanden**; anschließend ausschließlich die unveränderten `RUSTSEC-2026-0543`/`RUSTSEC-2026-0555`. Bans, Lizenzen und Quellen grün (`/tmp/solid-107/lifecycle-check-all.log`). Wegen `set -e` Frontend/E2E separat wie oben geprüft. Commit mit `git log` feststellen. Plattformabhängigkeiten, Rust, Schema, API und Freigaberegeln unverändert.
- **Weiter offen:** #113/#119 benötigen weiterhin das grüne Gesamtgate. Keine Chrono-Ausnahme oder Backend-Migration ohne eigenen Auftrag. Vor Übernahme fehlt außerdem der unabhängige Integrationsreview; weder die Zahl bestandener Tests noch diese gezielten Nachweise ersetzen ihn. Keine GitHub-Schreiboperation, kein Push/Merge/Release.

## 0b. Fortsetzung: verbleibender Lifecycle-Fehler und Browser-Fehlerpfade

### Auftrag und Commit-Grenze

- Der Nutzer hat die Fortsetzung mit Codemode und `implement-spec` sowie issuebezogene Commits ausdrücklich beauftragt. Die frühere fehlende Commit-Freigabe ist damit überholt; ein grünes Integrationsgate oder ein Backend-Umbau ist dadurch nicht freigegeben.
- Alle Tickets #107–#119 erneut gelesen. #109–#112 erlauben ausdrücklich geprüfte Zwischencommits auf dem Integrationszweig auch mit dokumentierten Integrationsresten. Vor jedem Commit wird trotzdem `./scripts/check-all.sh` ausgeführt; #113/#119 bleiben bei rotem Gesamtgate offen.
- Keine verfügbaren Subagent-Werkzeuge: sequenzielle Bearbeitung auf dem vorhandenen `integration/solid-2`, keine vorgetäuschten Worktree-Merges oder unabhängigen Reviews. Keine GitHub-Schreiboperation, kein Push/Merge/Release. Sachfremde Dateien bleiben außerhalb der Commits.
- Die bestehenden Änderungen sind bereits über die Migrations- und Vereinfachungstickets hinweg verflochten. Commit-Aufteilung deshalb nach zusammenhängenden Modulen: gemeinsame Plattform (#109/#118), Dateien (#110/#114/#116), Aktivität (#111/#115/#117), übrige Ansichten (#112/#118), Bereinigung und Nachweise (#108/#113/#119). Kein Ticketabschluss allein aus einem Commit ableiten; tatsächliche Commits mit `git log 7a21786..HEAD --oneline` prüfen.

### Neue Umsetzung und Belege

- **#111/#115, echter Lifecycle-Fehler:** Eine verspätete Antwort auf höhere Kodierfreigabe rief nach Verlassen der Jobdetails weiterhin `onChanged`/`onRaised` auf und konnte dadurch zu einem fremden neuen Job navigieren. Neuer Test in `activity.test.tsx` reproduzierte den Fehler zuerst rot; ein lokaler Disposal-Guard in `RaiseEncode.tsx` verhindert die Seiten-Callbacks nach Unmount. Kein Versprechen eines Transportabbruchs oder einer serverseitigen Rücknahme.
- **#110/#114, Browser-Fehlerpfad:** `solid-2-inspector-errors.spec.ts` prüft einen Inspektionsladefehler und eine verzögerte Wiederholung mit echten Serverdaten. Dateidetails und offene Re-Inspektionsbestätigung bleiben erhalten, einschließlich derselben fokussierten Bestätigungsschaltfläche; Abbrechen gibt den Fokus an den Auslöser zurück.
- **#115, Browser-Nachladefehler:** `solid-2-item-errors.spec.ts` prüft Fehler und verzögerte Wiederholung der zweiten Item-Seite über kontrollierte HTTP-Antworten. Die erste Zeile, das offene Protokoll und dessen fokussierter Schalter bleiben erhalten; die erfolgreiche Wiederholung ergänzt genau das zweite Item.
- **Gesamtprüfung erstmals wieder vollständig bis Advisory-Gate:** `/tmp/solid-107/next-check-all.log`: Rust-Format, Clippy und **944 Rust-Tests in 71 Testprogrammen bestanden**. Anschließend unverändert ausschließlich `RUSTSEC-2026-0543` und `RUSTSEC-2026-0555`; Bans/Lizenzen/Quellen bestehen. Keine Advisory-Ausnahme. Wegen `set -e` überspringt das Gesamtgate Frontend/E2E; diese sind separat geprüft.
- Normales `npm ci` und `npm ls --all` nach den Änderungen bestanden (`next-ci.log`, `next-deps-clean.log`). Weiterhin einzige Vorabversion: freigegebene Testing Library 1.0.0-rc.2 (`next-prereleases.log`). Produktions-Modulgraph erneut geprüft (`next-production.log`).
- Typecheck und unverdeckter Development-Gesamtlauf: **783 Tests in 43 Dateien bestanden**, keine Solid-`[CODE]`-Diagnosen (`next-tests.log`). Produktionsbuild und **33 Chromium-Journeys bestanden** (`next-e2e-full.log`). Einzelne jsdom-Nichtimplementierungen bleiben unverdeckt.
- Bereits erstellt: `f28eadd` (#109, gemeinsame Plattform/#118), `f5c1a67` (#110, Dateien/#114/#116), `3fb8c7c` (#111, Aktivität/#115/#117), `0d44437` (#112, übrige Ansichten/#118). Vor jedem Commit eigener Gesamtprüflauf mit jeweils 944 bestandenen Rust-Tests und ausschließlich den zwei bekannten Advisory-Fehlern. Die abschließende Bereinigung samt Nachweisen gehört zu #119; ihren Commit mit `git log` feststellen.
- Gesamtprüfungen für die Commits werden getrennt unter `/tmp/solid-107/commit-*.log` protokolliert. Konkrete Commit-Zuordnung und Gate-Befund stehen zusätzlich in den Commit-Nachrichten. Kein Commit bedeutet Ticketabschluss oder grünes Integrationsgate.

### Weiter offen

- #113/#119 sind **kein grünes Gesamtgate**: Chrono/Chrono-tz bleiben separat blockierend. Die fehlende aktuelle Rust-Testbestätigung ist dagegen jetzt nachgeholt.
- Die zwei Browserkombinationen aus dem vorherigen Arbeitsauftrag sind nun belegt. Weitere Mutations-/Navigationskombinationen bleiben anhand der Kriterien zu bewerten; diese Fortsetzung ist keine Behauptung universeller Lifecycle-Abdeckung oder unabhängiger Abschlussprüfung.
- Keine Backend-, Schema-, API- oder Freigabeänderung ohne eigenen Auftrag. Keine Issues als abgeschlossen ausweisen, solange das Gesamtgate rot ist.

## 0a. Weitere Fortsetzung: Diagnosen, Konkurrenz und Nachweise

### Grenzen

- Weiterhin `integration/solid-2`, HEAD `7a2178611943736b7393d16fd4ab2390ed26d3dc`, alle Änderungen uncommittet. Bestehende Änderungen und unversionierte Fremddateien bewahrt.
- Regeln, Skill samt Referenzen und installiertem Primer/Diagnosehandbuch sowie alle Issue-Kriterien #107–#119 per `gh issue view` erneut geladen. Keine GitHub-Schreiboperation, kein Commit/Push/Merge/Release.
- Rust-, Schema- und API-Code unverändert. Die Chrono-Advisories bleiben separat blockierend; keine Ausnahme. Die Gesamtprüfung wurde in dieser Fortsetzung nicht wiederholt. Ein aktueller vollständiger Rust-Testabschluss fehlt weiterhin.

### Neue Umsetzung

- **Development-Diagnosen ursächlich behoben:** API-/Filter-/Sortier-/Phasenwerte werden aus Effect-Compute an Apply übergeben. Spaltenantworten schreiben nur Einträge; ein separater Compute entscheidet mit aktuellem Navigationszustand über Wiederherstellung/ersten Eintrag. Vollständig restaurierter Pfad wird nach der letzten Antwort samt Filter-Snapshot gemeldet. Übersicht berechnet Statusfarben reaktiv. Zeitplan-Checkboxen weisen den lokalen Klick zurück, bis der Server beziehungsweise die Bestätigung den Schalter tatsächlich freigibt.
- **Keine Unterdrückung:** Temporäre Konsolen-Stackinstrumentierung vollständig entfernt; `tests/setup.ts` enthält nur das schon vorhandene Storage-Cleanup. Kein neuer `untrack`-Schalter, Warnfilter oder pauschaler Flush.
- **Datei-Refresh serialisiert:** Job-Polls stapeln sich nicht. Scan-Invalidierungen teilen einen laufenden Refresh, verlangen aber einen Anschlussread. Während Nachladen wird eine Invalidierung vorgemerkt und danach werden alle sichtbaren Seiten erneut gelesen. Filterwechsel invalidiert ältere Antworten. Zähler-/Summary-Antworten haben getrennten Schutz vor älteren Antworten.
- **Job-Refresh serialisiert:** Aktive und relative Polls teilen einen laufenden Read. Nachladen hat Vorrang; Generationen verhindern alte Filterantworten. Der neue kombinierte Test deckt langsame Antworten über Mitternacht und den Wechsel von verborgener Aktivität zum relativen Intervall ab.
- **Formulare:** Settings entfernen nach erfolgreicher Speicherung nur die tatsächlich bestätigten Entwurfswerte. Während des Requests weiterbearbeitete Felder bleiben für den nächsten Submit erhalten. Auditantworten werden generationengeprüft, damit alte Feldverläufe nicht den neuen Verlauf ersetzen. Aktions-/Moduswechsel bei Zeitplänen verwenden den kumulierten Draft, ebenso Uhrzeit plus unmittelbarer Cron-Wechsel. Alte Aktions- und Sprachvorschauen werden verzögert getestet verworfen.
- **Weitere synchrone Sperren:** Auth, Scan, verwaiste Dateien, fehlende Dateien, Jobbereinigung, Untertitel-Nachladen und Dateiauswahlaktionen. Vorhandene Idempotenz-/Bestätigungsverträge bleiben erhalten.
- **Lifecycle:** Neue Tests reproduzierten verspätete Inspector-/Jobmutations-Callbacks nach dem Verlassen. Disposal-Guards verhindern solche Seiten-Callbacks und nachfolgende Reads; auch Scan-/Bereinigungsantworten werden beim Verlassen abgegrenzt. Keine Behauptung eines echten Transportabbruchs oder einer serverseitigen Rücknahme.
- **Erste Jobzuordnung ohne Remount:** Der Disposal-Schutz machte einen bisherigen unnötigen Remount beim verspäteten ersten Zuordnen eines Deep Links sichtbar. Eine reine Memo-Zuordnung ersetzt nun den nachführenden `openedKey`-Effect und erhält dieselbe Detailinstanz beim ersten eindeutigen Listenfund. Direkte aktuelle IDs haben Vorrang. Entfernte/mehrdeutige Zuordnungen werden weiterhin verworfen, nicht später wiederbelebt. Ein deterministischer Test hält eine Retry-Antwort über die erste Listenantwort hinweg zurück.
- **Jobdetails bereinigt:** `firstPage` heißt entsprechend seinem tatsächlichen Mehrseitenvertrag `itemPages`; `itemsReady` entfernt. Lokale Item-Loading-Anzeige bleibt über das anfängliche `undefined` unabhängig vom Job-Erstladen; Seitenanzahl und Nachlade-/Revalidierungssteuerung bleiben begründet erhalten.

### Aktuelle Prüfungen

| Prüfung | Ergebnis / lokaler Beleg |
| --- | --- |
| Erneutes normales `npm ci`, `npm ls --all` | Bestanden, `continue-ci.log`, `continue-deps-final.log` unter `/tmp/solid-107/` |
| Vollständiger Vorabversionsscan | Weiterhin ausschließlich `@solidjs/testing-library 1.0.0-rc.2` |
| Typecheck | Bestanden nach den letzten Code-/Teständerungen |
| Unverdeckte Development-Tests | **782 Tests in 43 Dateien bestanden**, `continue-tests-final-3.log`; keine Solid-`[CODE]`-Diagnosen |
| Produktionsbuild | Bestanden im vollständigen E2E-Lauf, `continue-e2e-final-2.log` |
| Produktions-Modulgraph | Bestanden im letzten Codezustand, `continue-production-final.log` |
| Chromium-E2E | **31 Journeys bestanden**, 21,6 s, `continue-e2e-final-2.log` |
| Diff-Whitespace / Rust-Diff | Bestanden / keine Änderungen |
| Vollständige Projektprüfung | **Nicht grün**, unveränderter separater Rust-Blocker und fehlender neuer vollständiger Rust-Testabschluss |

Gegenüber dem Eintrittsstand sind **18 Verhaltenstests und drei Browser-Journeys ergänzt**, weitere vorhandene Tests auf wirklich synchrone Doppelauslösung verschärft. jsdom-Meldungen zu nicht implementierten Browserfunktionen bleiben unverdeckt und werden von Solid-Diagnosen getrennt bewertet.

Die neue Datei `frontend/e2e/solid-2-regressions.spec.ts` prüft Dateizeilen/Fokus bei frischen Objekten und Reorder, offene Joblogs/Fokus/aktuelle Requests bei kontrolliert wechselnden Job-/Item-IDs und Weiterbearbeitung während verzögerter Speicherung. Für echte GET-Serverantworten reicht Playwrights APIRequestContext Browser-Fetch-Metadaten nicht automatisch durch; der Test sendet den tatsächlichen Seiten-Origin mit. Keine Backend-Sicherheitsausnahme. Der echte Server lieferte im Dateitest stabile IDs; daraus wird ausdrücklich kein Browsernachweis wechselnder Datei-IDs behauptet.

### Weiterhin offen

1. Die **[vollständige lokale Kriterienzuordnung](solid-2-acceptance.md)** nennt Belege und Grenzen je Issue. Insbesondere Browserkombinationen aus Detail-/Nachladefehlern und Fokus sowie noch nicht einzeln verzögert getestete Mutations-Lifecycles weiter prüfen.
2. Die notwendigen Seiten-/Generations-/Refresh-Sperren nicht ohne Ersatznachweis entfernen. Die deutlich kürzere Jobzuordnung ist getestet; dies rechtfertigt keine pauschale Entfernung aller Identitätshilfen.
3. **Kein grünes #113-/#119-Gesamtgate:** Rust bleibt separat blockiert. Keine Advisory-Ausnahme und keine Backend-Migration ohne Auftrag. Kein Issue schließen und kein Commit vor gesonderter Freigabe samt vollständiger Projektprüfung.

**Historischer Wiedereinstieg dieses Abschnitts: Abschnitt 0a und Kriterienmatrix; aktueller Wiedereinstieg ist Abschnitt 0b.** Der nachfolgende Abschnitt 0 und seine offenen Punkte sind historisch und durch die obigen Nachweise teilweise erledigt.

## 0. Fortsetzung mit Codemode: historischer Stand 05:45 CET

### Grenzen und Eintrittskontrolle

- Projektregeln, Solid-2-Skill samt Referenzen und installierter Primer gelesen; Kriterien und Kommentare aller GitHub-Issues #107–#119 erneut per `gh issue view --json title,body,state,comments` geladen. Alle bleiben OPEN, ohne Kommentare oder Statusänderungen durch diese Sitzung.
- Weiterhin `integration/solid-2`, unveränderter HEAD `7a2178611943736b7393d16fd4ab2390ed26d3dc`. Alle Änderungen uncommittet; kein Checkout, Reset, Push, Merge, Release oder Issue-Abschluss.
- Vorhandene unversionierte Fremddateien aus Abschnitt 2 unangetastet. Kein Backend-, Schema- oder API-Umbau, keine Sicherheitsausnahme.
- Codemode für gebündelte unabhängige Aufrufe, sequenzielle Änderungen und verdichtete Testergebnisse eingesetzt.

### In dieser Fortsetzung umgesetzt

**Abhängigkeiten / #109 / #113 / #119**
- Vite **8.1.7** mit regulär passendem Rolldown **1.1.8** ersetzt Vite 8.3.8 / Rolldown 1.2.19. Vite verlangt `~1.1.7`; der WASM-Zweig verwendet stabile Emnapi-Versionen **1.11.1**.
- Lockfile isoliert unter `/tmp/solid-107/resume-deps` mit npm 11.21.3 neu aufgelöst; keine Overrides oder Peer-Ausnahmen. Danach normales `npm ci` mit npm 10.9.8 bestanden.
- Vollständige Lockfile-Prüfung einschließlich optionaler Plattformpakete: **einzige Vorabversion ist @solidjs/testing-library 1.0.0-rc.2**. `npm ls --all` besteht.
- Erzwungene Development-Conditions aus `frontend/vite.config.ts` entfernt. Das Plugin entscheidet nach Build-/Testmodus. Ein zusätzlicher Build mit Modulgraph-Prüfung bestätigt `solid-js/dist/solid.js`, `@solidjs/web/dist/web.js` und Signals unter `dist/prod/`, ohne Dev-/Server-Runtime.
- Beide CI-Jobs verwenden explizit Node **22.22.3** (Manifest weiterhin >=22.12.0).

**Identität / #115–#117**
- Neue kleine Helfer: `frontend/src/lib/rowKeys.ts` und `frontend/src/features/files/identity.ts`. Nur vollständige und innerhalb der aktuellen Liste eindeutige Fachschlüssel verbinden Antwortobjekte; sonst Objektidentität.
- Dateizeilen sind nach **(root_id, display_path)** statt mediathekübergreifend nur nach Pfad keyed. API-IDs bleiben aktuell. Auswahl, Cursor und Anchor werden bei Refresh nur eindeutig zugeordnet; mehrdeutige/entfernte Auswahl fällt weg.
- Jobzeilen verwenden vollständige, auf Eindeutigkeit geprüfte Kandidaten (Zeit, Aktion, Auslöser, Umfang, Motiv). Es gibt **keine garantierte natürliche Job-ID im API-Vertrag**; fehlende oder doppelte Kandidaten verbinden niemals verschiedene Antwortobjekte.
- Aktueller Job und gültige API-ID sind getrennt. `JobDetail.identity` begrenzt lokalen Zustand; dieselbe Jobidentität behält offene Logs auch bei geänderter Request-ID. Eine mehrdeutige Zwischenantwort verwirft die gemerkte Zuordnung dauerhaft, statt später einen beliebigen verbliebenen Job wieder als geöffnet zu markieren.
- Itemschlüssel verwenden nur nichtnegative, sichere, eindeutige Positionen innerhalb der Jobinstanz; lineare Vorberechnung statt quadratischem Filtern pro Item. Redundante innere Job-Key-Hülle entfernt.
- DOM-Zeilen bleiben erhalten; beim Umsortieren verlorenen Fokus stellt ein lokaler `onSettled`-Schritt nur für dasselbe noch verbundene Element und nur bei auf den Body gefallenem Fokus wieder her.
- Jobdetails lesen nach Item-Nachladen bei Revalidierung **alle sichtbaren Seiten** erneut. Nachladen und explizite Item-Revalidierung sind gegeneinander geschützt. Eigene Seitenanzahl bleibt für diesen Vertrag nötig.
- Synchrone Nachlade-Sperren für Items, Dateien und Jobs; Aktivitäts-Nachladefehler behalten bereits sichtbare Jobs und erlauben Wiederholung.
- `frontend/src/lib/stable.ts` und `frontend/tests/stable.test.ts` entfernt: keine produktiven Aufrufer mehr, keine rekursiven ID-ignorierenden Vergleiche oder alten IDs in frischen DTOs.

**Zustand und Diagnosen / #118–#119**
- Einstellungen und Zeitplanaktionen verwenden synchrone In-flight-Sperren statt ausschließlich verzögert übernommener Busy-Signale.
- Wochentagsänderungen verwenden den vorherigen Draft im Setter; zwei synchrone Änderungen verlieren keinen Tag. Settings-Draft-Updater unverändert erhalten; kein unbegründeter Store-Umbau.
- Zusätzliche Sperren für Vorbereiten-Vorschau und höhere Kodierfreigabe.
- Listenlade-Effects geben Filter-/Sortiersnapshots aus Compute an Apply weiter. Weitere Top-Level-Reads in Vorbereiten-Vorschau, Inspector-Bestätigung und höherer Kodierfreigabe ursächlich korrigiert, ohne Diagnosefilter.

### Aktuelle Prüfbelege

| Prüfung | Ergebnis |
| --- | --- |
| Normales npm ci / npm ls --all | bestanden |
| Typecheck | bestanden; zuletzt vor dem letzten vollständigen Frontend-Lauf |
| Frontend, ohne Konsolenabfang | **764 Tests in 43 Dateien bestanden**, `/tmp/solid-107/resume-final-tests-2.log` |
| Produktionsbuild | bestanden, `/tmp/solid-107/resume-final-build-2.log` |
| Produktions-Modulgraph | bestanden, `/tmp/solid-107/resume-production.log`; Prüfskript `verify-production.mjs` im selben temporären Verzeichnis |
| Chromium-E2E | **28 bestanden**, 14,6 s, `/tmp/solid-107/resume-final-e2e-2.log` |
| Diff-Whitespace | bestanden |
| Vollständige Projektprüfung | **nicht abgeschlossen**: zwei Werkzeug-Zeitlimits nach 180 / 600 s während Rust-Tests, `resume-check-all.log` / `resume-check-all-2.log` |
| Cargo deny, separat | **fehlgeschlagen**, dieselben zwei ungepflegten Chrono-Pakete; `/tmp/solid-107/resume-deny.log` |

Gegenüber 750 Tests wurden **19 Verhaltenstests ergänzt** und **5 obsolete Helfertests entfernt**. Die 764 Tests sind keine vollständige Kriterienabnahme. Rust-Format und Clippy erreichten in beiden Gesamtversuchen den Testschritt; ein neuer vollständiger Nachweis der 944 Rust-Tests liegt **nicht** vor. Der zweite Lauf endete zuletzt bei `schedule_calendar`; keine Testfehlermeldung, aber auch kein Abschluss. Daraus weder einen spezifischen hängenden Test noch einen Backend-Defekt ableiten. Keine zugehörigen Test-/E2E-Server laufen weiter.

### Weiterhin offen, als Nächstes bearbeiten

1. **Diagnosen bleiben echte Integrationsreste.** `npm test -- --disableConsoleIntercept` zeigt trotz grüner Tests weiterhin `STRICT_READ_UNTRACKED` (insbesondere Spaltenansicht: `restoring`, `chain`, `sel`, `trail`, außerdem indirekte Props-/Filterreads in Apply) und `UNCOMMITTED_INPUT_OVERWRITTEN` bei Checkboxen. Nicht mit einem Grep auf den normalen Vitest-Lauf für erledigt erklären. Keine globalen Warnfilter oder pauschalen untrack-Hüllen.
2. **Listen-Refresh-Konkurrenz prüfen:** Datei-Polling/Scan-Ereignisse und aktive/relative Job-Polls können noch verschiedene Refresh-Wege auslösen. Generationen und Seitenanzahl nicht ohne zusätzliche verzögerte-Antwort-Tests entfernen. Hintergrund-Refresh und Nachladen mit wechselnden Filtern, Fehlern und langen Antworten vollständig abnehmen.
3. **Formulare weiter abnehmen:** gleichzeitige unabhängige Feldänderungen und Doppelspeicherung sind ergänzt; Änderungen des Entwurfs während laufender Speicherung, überholte Auditantworten sowie schnelle abhängige Änderungen von Aktion/Modus/Feldern und Zeitplanvorschauen weiter prüfen. Signal-only-Sperren in anderen Mutationen (z. B. Scan, Auth, verwaiste Dateien, Untertitel-Nachladen, Dateiauswahlaktionen) noch gezielt prüfen.
4. **Async-Bereinigung:** `itemsReady` ist noch ein einfacher Alias auf `firstPage() !== undefined`; das Memo lädt mittlerweile alle sichtbaren Item-Seiten. Benennung/Loading-Grenze und stille Fehler-/Sitzungsrevalidierung sauber prüfen. Inspector-/Item-Nachladefehler und Fokusverhalten zusätzlich im Browser absichern; neue Identitätsszenarien wurden bislang in jsdom geprüft.
5. Vollständige Issue-Kriterien-Zuordnung für #110–#119 und ergänzte Browser-Journeys fehlen. Bestehende 28 Journeys bleiben grün, decken aber nicht jede neue Identitäts-/Konkurrenzkombination ab.
6. **Rust separat:** bekanntes Advisory-Gate weiterhin rot; keine sichere Upgrade-Version, keine Freigabe für Migration/Ausnahme. Die Zeitlimits der Gesamtprüfung kommen als fehlender aktueller Rust-Testabschluss hinzu. Backend nicht stillschweigend umbauen.

**Nächster Prozess beginnt mit diesem Abschnitt**, dem aktuellen Git-Diff und den offenen Issue-Kriterien, nicht erneut mit dem inzwischen bereinigten Emnapi-Blocker. Die nachfolgenden ursprünglichen Abschnitte dienen als historische Baseline.

## 1. Im neuen Prozess beginnen

```bash
cd /Users/chl/gitea-repositories/atvprep-rs
pi --tools +codemode
```

`.pi/settings.json` enthält bereits `"defaultTools": ["+codemode"]`. Die Datei ist unversioniert und wurde vom Nutzer während der Sitzung angelegt. Codemode war zuletzt verfügbar und wurde erfolgreich eingesetzt. Es werden keine weiteren MCP-Server oder Modellzugänge benötigt.

Kopierbarer Startauftrag:

> Setze die pausierte SolidJS-2-Migration im bestehenden Arbeitsbaum auf `integration/solid-2` fort. Lies zuerst `docs/solid-2-handoff.md`, die Projektregeln und den Skill `solidjs-2`. Auftrag sind GitHub #107 und die Teilissues #108–#119 in `chLuethy/atvprep-rs`; lade die jeweiligen Abnahmekriterien mit `gh issue view`. Nutze Codemode für unabhängige Werkzeugaufrufe und verdichtete Prüfergebnisse. Bewahre sämtliche vorhandenen Änderungen. Beginne mit den offenen Abhängigkeits- und Integrationsproblemen aus der Übergabe; arbeite danach die verbleibenden Vereinfachungen und Nachweise ab. Der Rust-Blocker wurde separat untersucht und soll weder stillschweigend umgebaut noch durch Ausnahmen umgangen werden. Keine Commits, Pushes, Merges, Releases oder Issue-Schließungen ohne gesonderten Auftrag.

### Eintrittskontrolle

1. `AGENTS.md`, `CODING_STANDARDS.md`, `docs/agents/codemap.md` und den Solid-2-Skill lesen.
2. `git status --short`, `git branch --show-current` und den Diff prüfen. **Kein neuer Checkout und kein Zurücksetzen des Arbeitsbaums.**
3. Die offenen Issue-Kriterien abrufen: `gh issue view <Nummer> --repo chLuethy/atvprep-rs --comments`.
4. Aktuelle Prüfungen wiederholen, soweit benötigt; temporäre Logs sind nur lokale Belege und könnten fehlen.

**Eintritt abgeschlossen, wenn** Arbeitsbaum und Ticketumfang verstanden, fremde Dateien geschützt und die beiden offenen Abhängigkeitsblocker ausdrücklich erfasst sind.

## 2. Arbeitsbaum und Grenzen

- Repository: `/Users/chl/gitea-repositories/atvprep-rs`.
- Branch: `integration/solid-2`.
- HEAD / Ausgangscommit: `7a2178611943736b7393d16fd4ab2390ed26d3dc`.
- Sämtliche Migration ist bislang **uncommittet**. Kein Push, Merge, Release oder GitHub-Issue-Abschluss.
- Sprache für neue Texte, Dokumentation und Kommentare: Deutsch; Code und Bezeichner Englisch.
- Keine Änderungen an Backend, Schema, API-Verträgen, Kodierfreigaben, Planner, Produktverhalten oder Gestaltung.
- Primärer Test-Seam bleibt das vorhandene injizierbare `api.request`; keine neue produktive Testschnittstelle.
- Vor jedem Commit/Merge ist `./scripts/check-all.sh` vorgeschrieben. Ein roter Rust-Check darf nicht als vollständig grünes Integrationsgate ausgewiesen werden.

Folgende bereits vorhandene unversionierte Dateien/Verzeichnisse erhalten:

- `.claude/`, `.playwright-mcp/`, `.pi/`
- `docs/release-notes/0.6.1.md`, `docs/release-notes/0.7.0.md`
- `scripts/umzug-github.sh`

Die Migrationsdokumente `docs/solid-2-migration.md`, `docs/rust-chrono-advisories.md` und diese Übergabe gehören zum aktuellen Arbeitsstand und sind ebenfalls noch unversioniert.

## 3. Tickets: lokaler Stand, nicht GitHub-Abschluss

Alle Issues waren bei der Pause auf GitHub **OPEN**. Kriterien erneut prüfen; die Tabelle ist keine pauschale Erledigtmeldung.

| Issue | Titel / Gegenstand | Lokaler Stand |
| --- | --- | --- |
| [#107](https://github.com/chLuethy/atvprep-rs/issues/107) | Gesamte Frontend-Migration und Vereinfachung | Offen; Integrationsbranch vorhanden, Gesamtziel nicht erreicht. |
| [#108](https://github.com/chLuethy/atvprep-rs/issues/108) | Kompatibilität klären und Verhalten absichern | Vorbereitungsumfang lokal umgesetzt: Matrix, Baseline, Ark-Nichtnutzung und vier Charakterisierungstests. Neu entdeckte transitive Vorabpakete müssen vor Gesamtfreigabe geklärt werden. |
| [#109](https://github.com/chLuethy/atvprep-rs/issues/109) | Toolchain, Einstieg, gemeinsame Bedienung | Weitgehend umgesetzt und laufend geprüft; Abhängigkeitsbaum, Produktionsbedingungen und explizite CI-Node-Untergrenze offen. |
| [#110](https://github.com/chLuethy/atvprep-rs/issues/110) | Dateien, Inspector, Spaltenansicht | Kompatibilitätsänderungen umgesetzt; bestehende Tests und E2E grün. Noch keine vollständige Kriterienabnahme. |
| [#111](https://github.com/chLuethy/atvprep-rs/issues/111) | Aktivität, Jobdetails, Logs | Kompatibilitätsänderungen umgesetzt; bestehende Tests und E2E grün. Noch keine vollständige Kriterienabnahme. |
| [#112](https://github.com/chLuethy/atvprep-rs/issues/112) | Übersicht, Scan, Einstellungen, Zeitpläne | Kompatibilitätsänderungen umgesetzt; bestehende Tests und E2E grün. Noch keine vollständige Kriterienabnahme. |
| [#113](https://github.com/chLuethy/atvprep-rs/issues/113) | Vollständige Plattformmigration integrieren und abnehmen | Offen: Frontend-Belege vorhanden, Gesamtgate durch Rust blockiert; zusätzlich transitive Vorabpakete ungeklärt. |
| [#114](https://github.com/chLuethy/atvprep-rs/issues/114) | Inspector-Laden und Revalidierung | Teilweise vorgezogen: Async-Memos, `refresh`, keyed Dateiinstanz und `Loading`. Vollständige Kriterienprüfung und Nachweise offen. |
| [#115](https://github.com/chLuethy/atvprep-rs/issues/115) | Jobdetails und stabile Items | Teilweise vorgezogen: Async-Memos, keyed Jobinstanz, Item-Keys, aktuelle Log-ID. Zusätzliche Identitäts-/Fehler-/Nachladetests und Bereinigung offen. |
| [#116](https://github.com/chLuethy/atvprep-rs/issues/116) | Stabile Dateizeilen und Listenaktualisierung | Offen; alte `reuseUnchanged`-Logik noch aktiv. Besonders Mediathek-Scope, aktuelle API-IDs und Auswahlzuordnung bearbeiten. |
| [#117](https://github.com/chLuethy/atvprep-rs/issues/117) | Stabile Jobs und Aktivitätsaktualisierung | Offen; alte Wiederverwendung und Fingerprints noch aktiv. Eindeutigkeit und aktuelle API-IDs absichern. |
| [#118](https://github.com/chLuethy/atvprep-rs/issues/118) | Filter-/Formularzustand gezielt vereinfachen | Teilweise vorgezogen: writable derived Signals für Filter, Suche und Sortierung. Formularprüfung, gleichzeitige Feldänderungen und Mehrfachauslösungen offen. |
| [#119](https://github.com/chLuethy/atvprep-rs/issues/119) | Restlogik bereinigen und Gesamtziel nachweisen | Offen; Helfer, Diagnosen, Abhängigkeiten, ergänzte Tests und Abschlussbericht fehlen noch. |

## 4. Letzter Prüfstand

| Prüfung | Letztes Ergebnis / Beleg |
| --- | --- |
| Frontend-Typecheck | Bestanden vor dem letzten vollständigen Testlauf (`npm run typecheck`). |
| Frontend-Tests | **750 bestanden in 44 Dateien**, keine unbehandelten Testfehler; `/tmp/solid-107/runtime-6.log`. |
| Produktionsbuild | Bestanden mit Vite 8.3.8; `/tmp/solid-107/build-1.log`; anschließend im E2E-Skript nochmals gebaut. Produktionsauflösung trotzdem gesondert prüfen, siehe unten. |
| Chromium-E2E | **28 bestanden**, 14,7 Sekunden Testlaufzeit; `./scripts/run-e2e.sh`, `/tmp/solid-107/e2e-1.log`. |
| Rust | Baseline: Format, Clippy und **944 Tests bestanden**; `cargo deny` schlägt fehl. Nach Frontend-Umstellung keine neue vollständige Projektprüfung ausgeführt. |
| Diff-Whitespace | Letzter `git diff --check`: **zusätzliche Leerzeile am Dateiende von `frontend/src/lib/stable.ts:56`**. Bei der Pause bewusst nicht mehr korrigiert. |

Es läuft nach der Pause kein zugehöriger Test-/E2E-Server weiter. Das E2E-Skript beendet seinen Server selbst.

750 grüne Tests sind überwiegend die vorhandenen 746 Tests plus vier neue Charakterisierungstests. Sie ersetzen **nicht** die ergänzten Szenarien aus #115–#118. Frühere rote Zwischenläufe `runtime-1` bis `runtime-5` sind keine aktuelle Fehlerliste.

jsdom meldet unter anderem nicht implementiertes `window.scrollTo`, Canvas und Dokumentnavigation. Davon getrennt gab es während der Migration `STRICT_READ_UNTRACKED`-Warnungen. Ihre vollständige Beseitigung ist **noch nicht nachgewiesen**: erfolgreiche Vitest-Tests können Konsolenausgaben verbergen; nicht aus einem leeren Grep auf Warnungsfreiheit schließen. Keine globalen Diagnosefilter oder abgeschwächten Assertions einführen.

## 5. Offene Blocker und unmittelbare Anschlussarbeit

### A. Unzulässige transitive Vorabpakete — zuerst klären

Der Nutzer hat **nur `@solidjs/testing-library@1.0.0-rc.2` exakt freigegeben**. Die aktuelle Anwendung enthält im Lockfile zusätzlich:

- `@rolldown/binding-wasm32-wasi@1.2.19` → `@emnapi/core@2.0.0-beta.2` und `@emnapi/runtime@2.0.0-beta.2`.
- Es handelt sich um einen optionalen WASM-Zweig, der lokal unter macOS nicht aktiv installiert ist. Das ist keine Freigabe und wurde bei der letzten vollständigen Lockfile-Prüfung erst entdeckt.
- Vite 8.3.8 verlangt `rolldown ~1.2.18`; 1.2.18 hat dieselben Beta-Abhängigkeiten.
- Bereits recherchiert: WASM-Binding 1.2.0 verwendet stabile Emnapi 1.11.2; 1.2.4/1.2.6 verwenden Alpha-Versionen. Vite 8.0.0 verlangt seinerseits Rolldown 1.0.0-rc.9 und ist daher kein geeigneter blinder Rückschritt.
- **Noch keine Lösung implementiert.** Eine vollständig kompatible stabile Kombination ermitteln und testen. Keine inkompatiblen Overrides, Lockfile-Manipulationen, `--force` oder `--legacy-peer-deps` verwenden. Wenn keine Lösung möglich ist, konkrete zusätzliche Freigabe erfragen statt die vorhandene Ausnahme auszuweiten.

### B. Produktionsauflösung und Node-Untergrenze

- `frontend/vite.config.ts` erzwingt momentan `resolve.conditions: ["development", "browser"]`, auch beim Produktionsbuild. Prüfen und so trennen, dass Tests Development-Diagnosen behalten und der Produktionsbuild die richtige Runtime erhält. Zuletzt etwa 487,59 kB JS unkomprimiert; keine Optimierungszusage daraus ableiten.
- `frontend/package.json` setzt Node `>=22.12.0`; `.github/workflows/ci.yml` verwendet an zwei Stellen weiterhin nur `'22'`. Explizite passende Untergrenze nach #109 nachziehen.

### C. Bekannter Rust-Blocker bleibt separat

Siehe `docs/rust-chrono-advisories.md`:

- `chrono 0.4.45`: `RUSTSEC-2026-0543`.
- `chrono-tz 0.10.4`: `RUSTSEC-2026-0555`.
- Beide `unmaintained`, keine sichere Upgrade-Version. Direkte aktive Nutzung in API, Manager und Fixtures.
- Nutzerauftrag war: zuerst separat untersuchen, dann mit Frontend weiterarbeiten. Untersuchung erledigt; **keine** Freigabe einer Backend-Migration oder Advisory-Ausnahme.
- Beleg: `/tmp/atvprep-107-rust-advisories.log`, ältere Gesamtprüfung `/tmp/atvprep-107-baseline.log`.

**Plattformprüfung abgeschlossen, wenn** die tatsächlich aufgelöste Kombination nur freigegebene Vorabpakete enthält, Production-/Testauflösung stimmen und Frontend-Typecheck, Tests und E2E weiterhin bestehen. Der separate Rust-Blocker bleibt in jeder Gesamtbewertung sichtbar.

## 6. Wegweiser für die restliche Umsetzung

### Bereits migrierte Mechanik

- `frontend/src/App.tsx`: statischer `createRouter`, Routenobjekte, Kontext und normale Anker.
- Web-/JSX-Imports unter `@solidjs/web`; neuer Vite-Compiler; Ark UI entfernt.
- Effects mit Compute-/Apply-Phasen; Lifecycle überwiegend `onSettled` mit zurückgegebenem Cleanup; Ref-Callbacks besitzen keinen Owner.
- Inspector und JobDetail: Async-Memos/`refresh`, Ergebniswerte explizit behandelt, keyed Instanzen beim Objektwechsel. Die vier neuen Tests in `inspector.test.tsx` / `activity.test.tsx` prüfen verspätete Antworten und zwei synchrone Bestätigungsklicks.
- Synchroner In-flight-Schutz für Re-Inspektion, Inspector-Vorbereitung und Job-Cancel/Retry ergänzt. **Andere Mutations-/Nachladeaktionen weiterhin auf Signal-only-Guards prüfen.** Verzögerte Signalübernahme ist keine synchrone Sperre.
- `ColumnsView.tsx`: Request-Snapshot mit Wertvergleich verhindert unnötiges Neuladen derselben Spalte nach Auswahländerungen. Ohne dies verlor Mehrfachauswahl ihre Einträge. Bestehende Auswahltests sind wieder grün.
- `LogViewer.tsx`: `itemId` und Terminalzustand sind eigene Memos. Sonst verursachte jede Item-Aktualisierung zusätzliche Log-Reads oder setzte das 3-Sekunden-Intervall vorzeitig zurück. Der Test für den letzten Log-Read beim Ende besteht jetzt.
- `FilterBar.tsx`: Fokusübergabe nach dem verzögerten DOM-Update; initiales Ref-Fokussieren respektiert bereits im Popover liegenden Fokus.
- Tests verwenden an gezielten Stellen `flush`, `waitFor` oder erwartete UI-Ergebnisse nach Updates. Die beiden Tests für synchrone Doppelbestätigung haben weiterhin keinen Flush zwischen den Klicks.

### #115–#117: Identität weiter bearbeiten

- `frontend/src/lib/stable.ts`: `loadedFor` bereits entfernt. `sameIgnoringIds`, `keepIfSame` und `reuseUnchanged` sind noch vorhanden; nur `reuseUnchanged` hat produktive Aufrufer (LibraryPage, ActivityPage). Die anderen beiden hängen an dieser Implementierung bzw. ihren Tests.
- **Problem:** `reuseUnchanged` ignoriert rekursiv IDs und trägt bei Änderungen alte IDs weiter. Das kann Requests auf veraltete API-IDs festhalten. Nicht als beabsichtigten neuen Vertrag übernehmen.
- `LibraryPage.refreshRows` verwendet nur `display_path` als Schlüssel, ohne Mediathek-Scope. `FileTable` verwendet noch Standard-`For` nach Objektidentität. Auswahl, Cursor und Anchor hängen an API-IDs.
- `ActivityPage` verwendet `jobKey = requested_at|action|trigger`, daneben einen `fingerprint` mit weiteren Feldern und `openedFingerprint`. Eindeutigkeit ist noch nicht belegt; Standard-`For` und alte Wiederverwendung sind aktiv.
- `JobDetail` nutzt ein joblokales `For` mit Item-Position nur bei eindeutiger Position, sonst Objektidentität. Die Prüfung ist derzeit pro Item quadratisch. Fehlende/ungültige Positionen, wechselnde API-IDs, Einfügen, Umsortieren, Entfernen und offene Logs ergänzend prüfen. Kein Itemzustand darf in einen anderen Job gelangen.
- API-IDs aktuell halten; UI-Identität separat modellieren. Auswahl/Fokus und richtige Requests über den bestehenden UI-/Request-Seam testen. Fehlende/doppelte Schlüssel müssen sicher auseinanderfallen statt falsche Zeilen zu verbinden.
- Generationen, Seitenanzahl und Poll-Sperren nur entfernen, wenn überholte Antworten, Nachladen und erneutes Lesen aller sichtbaren Seiten nachweislich gleich korrekt bleiben.

### #118–#119: Zustand und Bereinigung

- `facetFilter.ts`: Filter und Suchfeld sind writable derived Signals; alter nachführender Synchronisierungs-Effect auf Timer-Abbruch reduziert. Filter, Suche, Back und Ansichtsgedächtnis gemeinsam prüfen.
- `LibraryPage`-Sortierung ist writable derived; `viewMemory.ts` bleibt der einzige Speicherzugriff für Ansichtsstände.
- Formular-Gesamtobjektkopien nur bei belegtem Nutzen durch Stores ersetzen. Insbesondere mehrere Feldänderungen im selben Ablauf, Settings-Revisionkonflikte und überholte Zeitplanvorschauen prüfen.
- Überflüssige Readiness-Hüllen (`itemsReady`, Alias-Memos etc.), redundante keyed Hüllen, veraltete Hilfertests und Kommentare gezielt bereinigen. Bestehende Ergebnisfehler nicht mit `Errored` verwechseln.
- Reaktivitätswarnungen an der Ursache beheben; insbesondere Effects, die im Apply-Callback andere reaktive Werte lesen, und Top-Level-Reads in Flow-Callbacks prüfen. Keine pauschalen `untrack`-Umhüllungen zur Beruhigung der Diagnose.

**Vereinfachung abgeschlossen, wenn** jedes Kriterium der betroffenen Tickets mit aktuellen Tests/Codebelegen zugeordnet ist und notwendige Restmechanik erklärt ist. Danach gemeinsame Abschlussprüfung und Migrationsbericht aktualisieren.

## 7. Toolchain, Quellen und Reproduktion

Aktuell installiert: Solid/Web 2.0.3, Router 2.0.0, Plugin 3.0.1, Compiler 2.0.2, Vite 8.3.8, **Vitest 4.1.12**, jest-dom 6.6.3, TypeScript 5.7.3, jsdom 25.0.1, `@types/node` 22.20.5. Lokal Node 22.22.3, npm 10.9.8. Die historische Kandidatenmatrix nennt noch Vitest 5.1.1; das ist nicht die aktuelle Anwendungsversion. Vitest 4 wurde für passende Matcher-Typen gewählt, `tests/setup.ts` importiert `@testing-library/jest-dom/vitest`.

npm 10.9.8 brach bei sauberer Neuauflösung der Vitest-4-Kombination mit einem internen Arborist-Fehler (`edgesOut`) ab. Die Lockfile-Auflösung erfolgte isoliert in `/tmp/solid-107/app-deps-v4` mit `npx --yes npm@11.21.3 install --package-lock-only`, ohne erzwungene Peers. Anschließend bestand normales **`npm ci` mit npm 10.9.8** in `frontend`. Ein neuer Installationsversuch darf den aktuellen Baum nicht unkontrolliert neu auflösen.

```bash
cd frontend
npm ci
npm run typecheck
npm test
npm run build
cd ..
./scripts/run-e2e.sh
./scripts/check-all.sh
```

Der letzte Befehl wird nach bisherigem Wissensstand am Rust-Advisory-Gate stoppen; Frontend/E2E daher zusätzlich separat prüfen. Keine Prüfung oder Assertion abschwächen.

Solid-2-Quellen: Skill `/Users/chl/.agents/skills/solidjs-2/SKILL.md`, installierte `frontend/node_modules/solid-js/skills/solid-2.0/SKILL.md`, Typdefinitionen und `CHEATSHEET.md`. Gepinnte offizielle Downloads liegen lokal unter `/tmp/solid-107/{MIGRATION.md,CHEATSHEET.md,SKILL.md,router.md}`; Quellcommit `68406c2077eff22c67f71db4a16bf7026ddceee1`. Weitere Quellen und Baseline stehen in `docs/solid-2-migration.md`.

Codemode-Dokumentation: `/Users/chl/.pi/agent/install/releases/1.5.2/node_modules/@earendil-works/pi-coding-agent/docs/codemode.md`. Unabhängige Aufrufe mit `Promise.allSettled` bündeln, lange Logs über Tools filtern und Exitcodes explizit auswerten. Änderungen derselben Datei und Installation/Build/Test auf gemeinsamem Zustand sequenziell halten. Kein Toolaufruf läuft parallel zu einem äußeren Codemode-Aufruf.
