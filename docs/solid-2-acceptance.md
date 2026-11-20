# Solid-2-Migration: lokale Kriteriennachweise #107–#119

Stand: 20. November 2026, 13:12 CET. Quelle sind die erneut mit `gh issue view --repo chLuethy/atvprep-rs --json title,body,state,comments` gelesenen Issues. Die Kriteriennummern folgen jeweils der Reihenfolge unter „Abnahmekriterien“. Alle Issues bleiben offen und unverändert. Dies ist eine lokale Nachweismatrix, **keine Betreiberabnahme und kein grünes Integrationsgate**.

## Prüfstand

| Prüfung | Ergebnis / lokaler Beleg |
| --- | --- |
| `npm ci` | Bestanden; npm 10.9.8, keine Peer-Ausnahmen; `/tmp/solid-107/next-ci.log` |
| `npm ls --all` | Bestanden; `/tmp/solid-107/next-deps-clean.log` |
| Vollständiges Lockfile einschließlich optionaler Plattformpakete | Einzige Vorabversion: ausdrücklich freigegebene `@solidjs/testing-library 1.0.0-rc.2` |
| `npm run typecheck` | Bestanden nach den letzten Testergänzungen |
| `npm test -- --disableConsoleIntercept` | **793 Tests in 43 Dateien bestanden**; `/tmp/solid-107/lifecycle-final-tests.log` |
| Solid-Development-Diagnosen | Keine `STRICT_READ_UNTRACKED`-/`UNCOMMITTED_INPUT_OVERWRITTEN`- oder anderen `[CODE]`-Diagnosen im obigen unverdeckten Gesamtlauf; keine Filter, Warnungsunterdrückung oder dauerhafte Konsoleninstrumentierung |
| Produktionsbuild | Bestanden, unter anderem im vollständigen E2E-Lauf; `/tmp/solid-107/next-e2e-full.log` |
| Produktions-Modulgraph | Bestanden; Core/Web aus Produktions-Entries, Signals aus `dist/prod/`; `/tmp/solid-107/next-production.log` |
| `./scripts/run-e2e.sh` | **33 Chromium-Journeys bestanden**, 23,5 s; `/tmp/solid-107/lifecycle-e2e.log` |
| `git diff --check` | Bestanden |
| Vollständige Projektprüfung | **Nicht grün, aber vollständig bis Advisory-Gate ausgeführt.** Format, Clippy und **944 Rust-Tests in 71 Testprogrammen bestanden** (`/tmp/solid-107/next-check-all.log`); anschließend ausschließlich die zwei bekannten Chrono-Advisories. Frontend/E2E wegen `set -e` separat geprüft. Vor den issuebezogenen Commits jeweils erneuter Lauf (`/tmp/solid-107/commit-*.log`); für die Lifecycle-Fortsetzung erneut 944 bestandene Rust-Tests und dieselben zwei Advisory-Fehler (`/tmp/solid-107/lifecycle-check-all.log`). |
| Rust-Advisory-Gate | Separat untersuchter, unveränderter Blocker: Chrono/Chrono-tz ohne sichere Upgrade-Version. Siehe [Untersuchung](rust-chrono-advisories.md). Keine Ausnahme und kein Backend-Umbau. |

jsdom meldet weiterhin nicht implementierte Browserfunktionen (`scrollTo`, Canvas, Dokumentnavigation). Das sind keine Solid-Reaktivitätsdiagnosen; diese Meldungen werden nicht gefiltert. Die Browser-Journeys laufen gegen den gebauten, vom Rust-Server ausgelieferten Client. Die fünf ergänzten Journeys verändern kontrolliert Antwortreihenfolge, Job-/Item-IDs, Fehlerstatus beziehungsweise Antwortzeit am HTTP-Seam, ohne produktive Testschnittstelle. Der Dateitest nutzt die echten Serverantworten mit stabilen IDs; der Job-/Logtest liefert bewusst kontrollierte DTOs mit wechselnden IDs. Echte Benutzerbedienung, Runtime und Browserfokus bleiben ungemockt.

Die temporären Logs sind lokale Belege und nicht Teil des versionierten Lieferumfangs. Reproduktionsbefehle stehen in [Migration](solid-2-migration.md) und [Übergabe](solid-2-handoff.md).

## #107: Gesamtziel

| Kriterium | Zuordnung und Grenze |
| --- | --- |
| 1: kompatibler vollständiger Abhängigkeitsbaum | `frontend/package{,-lock}.json`, `vite.config.ts`, `tsconfig.json`, CI; Installation, Baum und Modulgraph oben geprüft. Solid/Web/Signals 2.0.3, Router 2.0.0, Plugin 3.0.1, Compiler 2.0.2, Vite 8.1.7, Rolldown 1.1.8, Vitest 4.1.12, TypeScript 5.7.3. |
| 2: Benutzerabläufe und vollständige Projektprüfung | Frontend und 33 Browser-Journeys lokal grün; **Gesamtprojekt durch Rust blockiert**. Kein Abschluss von #107/#113/#119 ableitbar. |
| 3: eigene Bereitschaft in Details ersetzt oder begründet | Inspector: Async-Memos und `Loading`, keine Reload-Zähler. Jobdetails: Async-Memos, `Loading` für den Job; `itemPages` mit anfänglichem `undefined` und lokalem `Show` für die unabhängig ladbaren Items. Der redundante `itemsReady`-Alias ist entfernt. |
| 4: explizite Identität und begründete Resthelfer | `rowKeys.ts`, `files/identity.ts`, keyed `For`; `stable.ts` samt fünf Helfertests entfernt. Details unten bei #115–#117. |
| 5: tatsächlicher Bericht mit Restproblemen | Diese Matrix, Migrationsbericht und Übergabe; unten ausdrücklich benannte offene Nachweise. |

## #108: Vorbereitung

| Kriterien | Belege |
| --- | --- |
| 1–2: Baseline und Versionsmatrix | Historische Baseline und Primärquellen in `docs/solid-2-migration.md`; aktuelle Versionen und unabhängige Prüfungen oben. Rust-Baseline und aktuelle Nachweislücke getrennt. |
| 3–4: UI-Seam, verspätete Antworten, Mehrfachauslösung | `inspector.test.tsx`: Dateiwechsel mit verspäteten Details/Inspektionen/Jobs und synchroner Re-Inspektions-Doppelklick. `activity.test.tsx`: Job-/Item-Wechsel und synchroner Abbruch. Bestehendes `api.request`, keine gemockten Solid-Primitiven. |
| 5–6: Ark und Vorabversionen | Ark-Nichtnutzung dokumentiert, Abhängigkeit entfernt; keine aktiven Imports. Testing-Library-Ausnahme exakt erhalten, vollständiger Lockfile-Scan ohne weitere Vorabversion. |

## #109: gemeinsame Plattform

| Kriterien | Belege und Grenze |
| --- | --- |
| 1: Installation, Compiler, JSX, Runtime, Node | Obige Prüfungen; Manifest Node >=22.12.0, lokale und beide CI-Laufzeiten 22.22.3. Keine zwei Solid-Runtimes. |
| 2: Sitzung, Navigation, Palette, Hilfe, Sprache, Theme | `auth-gate`, `app-shell`, `ui`, `language`, `i18n`, `facet-filter` und Ansichtsgedächtnis-Tests; Browser `auth`, `smoke`, `files`, `jobs`, `view-memory`. Nicht jede Kombination von Hilfe/Theme/Sprache ist eine eigene Browser-Journey. |
| 3–4: Ownership/Lifecycle und gemeinsame Verträge | Split-Effects, `onSettled` mit Cleanup, keine Primitive in Ref-Callbacks; vollständiger Development-Testlauf ohne Solid-Diagnosen. Timer-/Fokus-/Flash-Tests bleiben erhalten. Keine dauerhafte Kompatibilitätsschicht. |
| 5–6: gestagte Writes, Mehrfachübermittlung, Integrationsreste | Neue synchrone Auth-/Scan-Tests; bestehende Tests für Formular-, Inspector-, Job- und Nachladeaktionen erweitert. Kein produktiver pauschaler `flush`. Verbleibende Integrationsgrenzen unten. |

## #110: Dateien und Spalten

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: Kompatibilität, Erstladen, Fehler, Sitzungs-/Dateiwechsel | `LibraryPage`, `Inspector`, `Sidecars`, `FixPreview`, `ColumnsView`; `library.test.tsx`, `inspector.test.tsx`, `columns-view.test.tsx`. Fehlerwerte werden ausdrücklich gerendert, nicht durch `Errored` ersetzt. |
| 3–4: Auswahl, Paginierung, Sortierung, Filter, Pfad und gestagte Writes | Listen-/Auswahltests, `column-path-memory.test.tsx`, Browser `files`/`view-memory`; neuer Test für Meldung des vollständig restaurierten Pfads. Spaltenantworten liefern Einträge; Navigation berechnet den nächsten Schritt in Effect-Compute und übergibt den Snapshot an Apply. |
| 5: Fokus, Bestätigungen, Mehrfachauslösung, aktuelle IDs | Inspector-Fokustests und synchrone Doppelbestätigung; Untertitel-Doppelnachladen; Listenidentitätstests mit wechselnden IDs. Neue Browser-Journey erhält ausgewählte DOM-Zeile und Fokus bei frischen Objekten und Reorder. **Wechselnde IDs sind dort nicht simuliert**, sondern in den deterministischen UI-Tests. |
| 6–7: Fachgrenzen und schrittweise Vereinfachung | Planer-/Kodierfreigabe-/Bestätigungstests und bestehende Browser-Journeys erhalten. Kein Backend-/Schema-/API-/Gestaltungsumbau. |

## #111: Aktivität, Details und Logs

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: APIs, Lifecycle und Jobwechsel | `activity.test.tsx`: verspätete Job-/Item-Antworten, eigene Jobinstanz, Logzuordnung. `activity-period-library.test.tsx`: Filter und Zeitgrenzen. |
| 3: aktive/verborgene Jobs und relative Zeiträume | Bestehende Tests für aktive Jobs, verborgene Jobs, „Today“ über Mitternacht und fortschreitende 24 Stunden. Neue Tests: kein Poll während langsamen Nachladens, beide sichtbaren Seiten danach revalidiert, verspäteter Poll nach Filterwechsel verworfen, kein weiterer Poll nach Unmount. |
| 4: Logs, Folge-Scrollen, letzter Read | `activity.test.tsx`: unabhängiger Retry, Scrollverhalten, geöffnete Logs, abschließender Read bei Terminalwechsel, Timer-Cleanup. Die neue Browser-Journey erhält das offene Log samt fokussiertem Log-Schalter bei wechselnden Job-/Item-IDs und prüft den Read der aktuellen Item-ID. |
| 5–6: Aktionen, Fokus, Idempotenz und Journeys | Cancel-/Retry-/RaiseEncode-Tests, synchrone Abbruchsperre, Fokusübergabe; `jobs.spec.ts`, `jobs-period.spec.ts`. Keine neue optimistische Erfolgsanzeige. |

## #112: Übersicht, Scan, Einstellungen und Zeitpläne

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: Kompatibilität, Lade-/Fehler-/Sitzungszustände | `overview`, `scan-control`, `settings`, `schedules`-Tests und entsprechende bestehende Browser-Journeys. Übersicht berechnet Statusfarben reaktiv statt im `For`-Setup. |
| 3: Validierung, Speichern, Revisionkonflikt und Entwurf | `settings.test.tsx`: Grenzen, Fehler, 409 mit Entwurfserhalt, gleichzeitige Feldänderungen, Doppel-Submit; neu: Weiterbearbeitung während Speicherung und überholte Auditantworten. Neue Browser-Journey bestätigt Weiterbearbeitung bei verzögerter PATCH-Antwort. |
| 4: Zeitplanbearbeitung und Vorschauzuordnung | Anlegen, Bearbeiten, Duplizieren, Pausieren, Bestätigen in `schedules.test.tsx`; neu: Uhrzeitänderung und Cron-Wechsel im selben synchronen Ablauf, verspätete Vorschau einer vorherigen Aktion wird verworfen. Die parametrisierte verzögerte Vorschauprüfung deckt Aktions- und Sprachwechsel ab. |
| 5–6: Servergrenzen, Bestätigungen, Idempotenz/Cleanup | Bestehende Limits-/Profil-/Freigabe-/Original-löschen-Tests; synchrone Wochentagsänderung und Doppel-Speicherung. Checkbox für Anschalten/Pausieren weist den lokalen Klick ausdrücklich zurück, bis Bestätigung/Serverantwort vorliegen. Scan und verwaiste Dateien besitzen synchrone In-flight-Sperren. Jetzt zusätzlich deterministisch geprüft: verspätete Scanabschlüsse, Ende einer Mehrmediathek-Scanfolge nach Verlassen, Bereinigung mit Erfolg/409 nach Verlassen sowie Zeitplan-Lauf, Pause, Löschen, Speicherung und Freigabefakten. Lebensdauersignal verhindert Zeitplan-Folgeaktualisierungen; Freigabefakten starten nach Verlassen keinen Dashboard-Read. Langsame Scan-Polls stapeln sich nicht. Kein pauschaler Nachweis für jede denkbare Navigation. |

## #113: Integrationsgate

| Kriterien | Belege und Grenze |
| --- | --- |
| 1: Integration | Gemeinsamer Arbeitsstand auf `integration/solid-2`; der Nutzer hat jetzt issuebezogene Zwischencommits freigegeben. Zusammenhängende Modul-Batches statt künstlicher nachträglicher Trennung bereits verflochtener Mechanik, kein Merge. Tatsächliche Commits: `git log 7a21786..HEAD --oneline`. Dies erfüllt noch kein grünes Integrationsgate. |
| 2: vollständige Prüfungen | Frontend, Produktionsbuild und E2E grün; **Rust-Gesamtprüfung bis zum Advisory-Gate abgeschlossen, aber wegen der beiden Chrono-Advisories nicht grün**. Dieses Kriterium bleibt blockiert. |
| 3: Altintegrationen, doppelte Runtime, Vorabversionen | Installation, vollständiger Baum-/Lockfile-Scan, Produktions-Modulgraph und Quelltextsuche geprüft. |
| 4–6: Navigation, Lifecycle, Bericht und Abgrenzung | Bestehende ansichtsübergreifende Browser-Journeys plus diese Matrix. Weitere unten genannte Randfälle sind keine pauschal ausgeschlossenen Regressionen. #113 bleibt kein grünes Gate. |

## #114: Inspector-Async

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: Async und ruhige Revalidierung getrennt vom Dateiwechsel | Async-Memos/`refresh` unter `Loading`; Dateiwechsel keyed, gleicher Dateiwechsel behält lokale Bedienung. Tests für unveränderte Revalidierung, einzelne Datenänderungen und vertauschte Antworten. |
| 3–4: Fehler, Wiederholung und Sitzung | Inspector-Tests für Initialfehler/Retry und Sitzung; zurückgegebene `kind`-Zustände weiterhin explizit. Neuer verzögerter Test: Während stiller Revalidierung bleiben die Details zunächst sichtbar; nach `SessionError` erscheint die Sitzungsmeldung statt des alten Erfolgsinhalts. |
| 5–6: Polling, Invalidierung, Bestätigung, Restmechanik | Tests für Fix-Polling, Versionsinvalidierung, Bestätigungsfokus und aktuellen Serverplan. Keine Transportabbruch-Zusage: Async-Memos verhindern alte sichtbare Antworten, brechen nicht automatisch jeden HTTP-Request ab. Neue Tests reproduzierten verspätete Mutations-Callbacks nach Unmount. Inspector und Jobdetails verhindern sie nun samt nachfolgenden Refetches. Das ist kein Rückgängigmachen einer bereits serverseitig gestarteten Mutation. |

## #115: Jobdetails und Items

| Kriterien | Belege und Grenze |
| --- | --- |
| 1: eigene Bereitschaft | `itemPages` liest alle bereits sichtbaren Seiten. `itemsReady` entfernt; unabhängige Item-Initialanzeige über `Show`, Job-Initialanzeige über `Loading`. Kein Reload-Zähler. |
| 2–4: joblokale eindeutige Items und aktuelle IDs | Sichere nichtnegative Positionen werden linear auf Eindeutigkeit geprüft; fehlende/doppelte Positionen fallen auf Objektidentität zurück. Ändern, Einfügen, Reorder, Entfernen, Jobwechsel und Log-Requests mit neuen IDs in `activity.test.tsx`. |
| 5–6: Async, Fehler, Aktionen, Nachladen und Cleanup | Detailfehler-/Sitzungstests, versetzte Job-/Item-Antworten, Nachlade-Doppelklick, Revalidierung aller sichtbaren Item-Seiten und Log-Timer-Cleanup. Neue Browser-Journeys `solid-2-inspector-errors.spec.ts` und `solid-2-item-errors.spec.ts` prüfen Inspektionsladefehler beziehungsweise Item-Nachladefehler mit verzögerter Wiederholung, erhaltener Bestätigung/Protokollansicht und echtem Fokus. Zusätzlich verhindert `RaiseEncode` nach rot/grün reproduziertem Test verspätete Seiten-Callbacks nach Verlassen der Jobdetails. |

## #116: Dateiliste

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–3: Geltungsbereich, keyed Liste und aktuelle IDs | Fachschlüssel `(root_id, display_path)` nur bei vollständiger, innerhalb der Liste eindeutiger Belegung; `For` keyed, DTOs unverändert frisch. Tests mit gleichen Pfaden verschiedener Mediatheken und Aktionen mit neuen IDs. |
| 4: Veränderungen und ungültige Schlüssel | UI-Tests für Änderungen, Einfügen, Reorder, Entfernen und fehlende/doppelte Schlüssel; Fokus/Auswahl erhalten nur eindeutige Zuordnungen. Browser-Fokus bei Reorder ergänzt. |
| 5–6: Seiten, Fehler und Konkurrenz | Bestehende Nachlade-/Fehlertests; neu: langsamer Job-Poll und Scan-Invalidierung werden serialisiert, eine Invalidierung während des Reads erzeugt einen Anschlussread; verspäteter Poll nach Filterwechsel wird verworfen. Keine gestapelten Vier-Sekunden-Polls. Ein zusätzlicher verzögerter Test merkt den Scan während des Nachladens vor, erhält beide Seiten und prüft anschließend die frischen IDs beider Seiten ohne Duplikate. |
| 7: Restmechanik | Generationen schützen Filter-/Sortierwechsel und konkurrierendes Nachladen. Seitenanzahl bestimmt den vollständigen sichtbaren Refresh. Refresh-Flight/Snapshot verhindern überlappende Writes; neue Scan-Ereignisse dürfen beim Zusammenfassen nicht verloren gehen. Diese Mechanik bleibt deshalb bewusst. |

## #117: Jobliste

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–3: Schlüsselprüfung, aktuelle IDs und Veränderungen | Kein garantierter natürlicher Schlüssel im API-Vertrag. Nur vollständige eindeutige Kandidaten aus Zeit/Aktion/Auslöser/Umfang/Motiv verbinden Objekte; sonst Objektidentität. Tests für Reorder, fehlende/doppelte Schlüssel, aktuelle IDs und mehrdeutige Zwischenantwort beim offenen Job. Die Zuordnung ist ein Memo statt eines nachführenden Effects; ein eigener verzögerter Deep-Link-/Retry-Test schützt die Detailinstanz beim ersten Listenfund. |
| 4–5: Gruppierung, Seiten und Polling | Alle sichtbaren Seiten werden neu gelesen; aktive und relative Polls teilen einen Refresh. Langsames Nachladen blockiert Polls, Filterwechsel verwirft alte Antworten; Tests oben. Bestehende relative Zeit-/Hidden-Active-Tests bleiben grün. Neuer kombinierter Test: Ein langsamer Read über Mitternacht wird nicht gestapelt; nach Ende verborgener Aktivität übernimmt das relative Intervall, einschließlich Cleanup nach Unmount. |
| 6: Wiederverwendung | Alte Objektwiederverwendung/Fingerprints entfernt; eindeutiger Kandidat bleibt als ausdrücklich begrenzte UI-Identität, niemals als Ersatz für die aktuelle Request-ID. |

## #118: Filter und Formulare

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: lokale Ableitungen/Stores nur bei Nutzen | `createFacetFilter`: beschreibbare Ableitung für Filter und Suchentwurf; Sortierung ebenfalls abgeleitet. Settings behalten den bereits kumulierenden Draft-Updater statt eines unbegründeten Store-Umbaus. |
| 3–4: Suche, Reset, Facetten, Ordnung, URL, Back, Erinnerung | Bestehende UI-/Adress-/Ansichtsgedächtnis-Tests und Browser `files`, `jobs`, `view-memory`; gespeicherter Zustand ausschließlich über `viewMemory.ts`. Spaltenpfad-Rückmeldung erhält den Filter-Snapshot ausdrücklich. |
| 5–6: Konflikte, Vorschau, gleichzeitige Änderungen/Mehrfachauslösung | Settings-Konflikttests plus neue Weiterbearbeitungs-/Audit-/Doppel-Submit-Tests. Zeitpläne verwenden kumulierende Updater auch beim abhängigen Aktions-/Moduswechsel; langsame alte Aktionsvorschau getestet. Verspätete Aktions- und Sprachvorschauen sind parametrisiert geprüft. |
| 7: entfernte/erhaltene Verträge | Nachführender Filter-/Such-/Sortierzustand reduziert; Debounce, URL-Kanonisierung und Ansichtsgedächtnis bleiben eigene fachliche Verträge. In-flight-Flags sind synchroner Request-Schutz, Busy-Signals nur UI-Zustand. |

## #119: Bereinigung und Abschluss

| Kriterien | Belege und Grenze |
| --- | --- |
| 1–2: obsolete Helfer und begründete Reste | `stable.ts` und Tests entfernt; `loadedFor`, Reload-Zähler und `itemsReady` entfallen. `firstPage` heißt jetzt seinem tatsächlichen Vertrag entsprechend `itemPages`. Restmechanik oben begründet. |
| 3: kompatible Installation | Erneutes normales `npm ci`, `npm ls --all`, vollständiger Vorabversionsscan, Produktions-Modulgraph. Keine weitere Ausnahme. |
| 4–5: Gesamtprüfung und Regressionen | **Nicht vollständig erfüllt:** Rust-Advisories blockieren weiterhin; Rust-Format, Clippy und alle 944 Tests sind nun aktuell bestätigt. Die benannten Browser-Fehler-/Fokuskombinationen sind ergänzt. 793 Tests/33 Journeys beweisen keine universelle Fehlerfreiheit; ein unabhängiger Integrationsreview steht weiterhin aus. |
| 6–7: Bericht und Grenzen | Versionen, Bereinigung, Restmechanik und Prüfungen hier dokumentiert. Keine Leistungsversprechen. Issuebezogene Zwischencommits sind jetzt ausdrücklich beauftragt; keine Pushes, Merges, Releases, Issue-Änderungen oder Schließungen. |

## Nächste gezielte Schritte

1. Die ergänzten Browsernachweise für Inspektionsladefehler und Item-Nachladefehler samt Fokus beibehalten. Bei weiteren Änderungen ausdrücklich zwischen diesen Fehlerpfaden und den bisherigen Erfolgs-/Identitätsjourneys unterscheiden.
2. Die nun ergänzten Lifecycle-Tests für Scan-/Bereinigungsantworten und Zeitplanaktionen erhalten. Vor Übernahme unabhängigen Integrationsreview nachholen; die gezielten Rot/Grün-Nachweise ersetzen ihn nicht.
3. Prüfen, ob die verbleibende Request-Steuerung danach noch lokal vereinfacht werden kann. Keine Generations-/Seitenlogik ohne Ersatznachweis löschen.
4. Rust-Gesamtgate bleibt separat zu entscheiden. Ohne Freigabe weder Advisory-Ausnahme noch Zeitbibliotheksmigration. Erst nach aktuellem vollständigem Projektlauf darf ein grünes Gesamtgate gemeldet werden.
