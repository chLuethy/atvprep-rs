# Wegweiser durch den Code

Einstiegspunkte nach Modul und Konzept, nicht nach Funktion. Suche den Namen im Modul selbst. Alle Pfade gelten vom Repo-Wurzelverzeichnis aus.

## Frontend (`frontend/src`)

- **Routen und Ansichtsgedächtnis.** `App.tsx` hängt die Routen ein. `Library` (Dateien), `Activity` (Aktivität) und `SchedulesRoute` (Zeitpläne) sind Routen-Wrapper: eine Route für Liste und geöffnetes Element, der Filter steht in der Adresse. `lib/viewMemory.ts` merkt den zuletzt gezeigten Ansichtsstand pro Seite und ist der einzige Zugriff auf den Browser-Speicher dafür.
- **Planerschritte.** `features/files/planSteps.ts` formatiert die Schritte, die der Server als Daten schickt (`PlanStep`, `formatPlanStep`); nichts liest Text zurück. `fixPlan.ts` und `preparePlan.ts` stellen darauf die erwarteten Änderungen dar.
- **Facetten-Filter.** `lib/facetFilter.ts` bündelt die Filtermechanik aller Listen: `useFilterAddress` (Adresse und Ansichtsgedächtnis, im Routen-Wrapper) und `createFacetFilter` (Suche mit Verzögerung, Schnellfilter, Abbildung auf die Filterleiste, Zurücksetzen, in der Seite) sowie die Adresshilfen `list`/`canonical`/`sorted`. Eine Liste liefert nur ihre `FacetFilterSpec` (`FILE_FILTER_SPEC` in `features/files/filters.ts`, `jobFilterSpec` in `features/activity/filters.ts`, `SCHEDULE_FILTER_SPEC` in `features/schedules/filters.ts`). Die Zeitplanliste filtert und sortiert weiter im Client.
- **Dateien und Spaltenansicht.** `features/files/`: `LibraryPage.tsx` (Liste), `ColumnsView.tsx` mit `browse.ts` (Ordner nachladen) und `columnPath.ts` (offener Pfad, über `viewMemory`), `filters.ts`, `loaders.ts`, `Inspector.tsx`. `preparePlan.ts` und `fixPlan.ts` stellen Planer-Ergebnisse des Servers dar.
- **Aktivität und Protokollansicht.** `features/activity/`: `ActivityPage.tsx` (Jobliste), `JobDetail.tsx` (Job und Items), `LogViewer.tsx` (Item-Protokoll), `filters.ts`, `loaders.ts`, `model.ts`.
- **Texte.** `lib/i18n.ts` und `locales/de*.ts`.
- **Farben, Breakpoint und Mindestversionen.** `styles/theme.css` ist die einzige Quelle der Farben (Seeds mit `light-dark()`, alle anderen Töne per `color-mix(in oklch, …)`). Das setzt Browser ab Chrome/Edge 123, Firefox 120 und Safari 17.5 voraus (`light-dark()`); `color-mix` und `oklch` brauchen weniger (Chrome 111, Firefox 113, Safari 16.2). Es gibt bewusst keinen Fallback im Code: ältere Browser zeigen die Seiten ohne Farbschema-Töne. Der Einspalten-Breakpoint 1180px steht als `SINGLE_COLUMN_MAX` in `lib/fitBottom.ts`; die CSS-Literale wiederholen ihn, `tests/theme-contract.test.ts` hält beide gleich. `public/favicon.svg` ist die einzige Datei mit Hex-Farben ausserhalb des Gerüsts (ein Favicon kennt keine CSS-Variablen); der Vertragstest erlaubt dort nur die hellen Seed-Werte.

## API (`rust/crates/atvprep-api/src`)

- **Lesepfade.** `read_files.rs` (Dateien, Ordner-Browse, Detail, Inspektionen) und `read_jobs.rs` (Jobs, Warteschlange, Items, Item-Protokoll), dazu `job_filter.rs` und `filter_params.rs` für die Abfrageparameter.
- **Facetten-Engine.** `facets.rs` ist die eine Engine für Dateiliste und Jobliste: eine `Facet` deklariert Adress-Schlüssel, Quelle der erlaubten Tokens (`Tokens`: fest, offen oder opake Ids aus der Datenbank) und SQL-Klausel; `FacetSet` parst die Adresse (`FacetError`), baut die Klauseln und zählt je Option den Filter ohne die eigene Facette. Die Dateiliste deklariert ihre Facetten in `read_files.rs` (`file_facets`), die Jobliste in `job_filter.rs` (Auslöser in `job_trigger.rs`); die Mediathek-Facette (`library_facet`) ist geteilt. Neue Facette: dort deklarieren, Zähler und Eingabeprüfung folgen.
- **Fehlerkonvention.** Fehler gehen als `ProblemDetail` aus `errors.rs` hinaus. Ein ungültiger Parameter oder Body ist 422 (`UNPROCESSABLE_ENTITY`, Code `invalid_request`).

## Kern (`rust/crates/atvprep-core/src`)

- **Planer.** `planner.rs`: `build_plan` entscheidet pro Datei, was geschieht, und erzeugt die ffmpeg-Argumente (`video_args`, `audio_args`, `subtitle_args`).
- **Audio-Analyse.** `analyzers.rs` (`analyze_audio`, dazu die Video- und Untertitel-Analyse) bewertet die Spuren, bevor der Planer plant.
- **FFmpeg-Lauf.** `prepare/ffmpeg.rs` startet das geplante Kommando und liest dessen Fortschritt; `prepare/pipeline.rs` ordnet die Stufen, `stages.rs` definiert die Maschinenzeilen auf stdout.
- **Chunk-Stufe.** `prepare/chunks.rs` (`Mp4Box`) schreibt die Chunks jeder ffmpeg-Ausgabe auf rund 0,5 s um (ADR 0010); `pipeline.rs` ruft sie zwischen ffmpeg und moov-Patch (`step_rechunk`, Stufe `chunks`). Fehlt MP4Box, scheitert der Lauf (`PrepareOptions::mp4box_required`, ErrorKind `mp4box`) oder der Hook warnt und liefert aus. Test-Naht: `RunContext::mp4box` (Suchverzeichnisse); Tests in `rust/crates/atvprep-core/tests/chunk_stage.rs`.

## Manager (`rust/crates/atvprep-manager/src`)

- **Worker-Ausführung.** `worker/execution.rs` ist die Hülle (Lease, Fencing, Permit, Zustandsübergänge des Items). `worker/action.rs` hält je Aktion (Vorbereiten, Prüfen, Analysieren) Argumente, Deutung des Prozessergebnisses und Protokollzeilen; Seams sind `ProcessAdapter` (echter Kindprozess oder festes Ergebnis), `Sink` und `Shell`. `worker/process.rs` startet die Prozesse.
- **Item-Protokoll.** `worker/logs.rs` (`ItemLog`, begrenzt, Schrittzeilen bleiben ganz), `worker/report_log.rs` und `worker/prepare_log.rs` (Schrittzeilen aus dem, was das Werkzeug meldet).
- **Zeitpläne.** `schedule_candidates.rs` (Kandidatenauswahl: die Zahl der API-Ansicht und die eingereihte Menge eines Laufs kommen aus derselben Abfrage), `schedule_due.rs` (reine Fälligkeitsentscheidung `decide`), `cron.rs` (der einzige Kalender), `calendar.rs` (Ortszeit eines Slots, Zeitzonen-Umrechnung `local_to_utc`, nächster Slot), `schedules.rs` (Dispatch und Lauf), `media_sql.rs` (gemeinsame SQL-Bausteine; Platzhalter, die mehrfach vorkommen, sind nummeriert `?N`). Die Aktion eines Zeitplans ist eine `JobAction` (`JobAction::from_schedule`).
- **Vorbereiten-Snapshot.** `prepare_snapshot.rs`: `PrepareSnapshot` ist der Typ der Einstellungen eines Vorbereiten-Jobs und seiner Items (Varianten manueller Auftrag und Zeitplan). Schreiber bauen ihn und rufen `to_json`; Leser parsen mit `parse` (benannte Ablehnung `Refusal`) oder lesen einen einzelnen Schlüssel mit `recorded_profile`/`recorded_encode`. Die JSON-Form ist Teil der Signatur (Golden `prepare_snapshot_*.json`). Nicht der Input-Snapshot (`snapshots.rs`).
- **Job-Aktion.** `state_machine.rs`: `JobAction` (Prüfen, Analysieren, Vorbereiten, Scannen) kennt gespeicherte Formen (`stored`, `stored_forms`, auch die historischen `fix`/`convert`), den öffentlichen Namen (`public`, `from_public`) und `changes_files`. Vergleiche mit Aktionsnamen gehören hierher, nicht als String in den Aufrufer.
- **Dateizustand.** `file_state.rs` entscheidet den Zustand einer Datei (Bereit, Einbettung offen, Warnung, …) an einer Stelle: SQL-Ausdruck `state_sql`, Rangfolge `RANKED`/`rank_sql`, Rust-Gegenstück `state_of`, dazu `is_preparable`. Filter, Sortierung, Spaltenansicht und Übersicht der API nutzen ihn; die Lese-API liefert `state` und `preparable` je Datei. Neuer Zustand: nur dort (Tabellentest) und in `frontend/src/features/files/filters.ts`.
- **Test-Daten.** `rust/crates/atvprep-fixtures` ist der Builder für Tests (`Fixtures`: Mediathek, Datei mit Befund, Job, Zeitplan, Sidecar, Inspektion). Er legt über die Repositories des Managers an; rohes SQL (`Fixtures::sql`) ist die begründete Ausnahme für historische oder ungültige Zeilen.
- **Migrationen.** `schema.rs` führt `migrations/000N_name.sql` der Reihe nach aus; `PRAGMA user_version` ist die Version. Neu: Datei anlegen, in `STAGES` anhängen, `VERSION` erhöhen.

## Golden- und Oracle-Daten (`rust/testdata/golden`)

Die JSON-Dateien halten das Verhalten des Python-Altsystems fest; Rust-Tests laden sie mit `load_golden` und müssen sie treffen. Eine gewollte Abweichung wird im Test markiert, die Datei bleibt unverändert:

- Ein Fall wird namentlich ausgenommen und eigens geprüft: `RESAMPLED_SINCE_83` in `rust/crates/atvprep-core/src/planner.rs` (der Oracle kopierte die Spur mit 44,1 kHz, seit #83 wird sie mit 48 kHz kodiert).
- Die Protokollzeilen der Aktionstests (`worker/action.rs`) liegen als Textdateien unter `rust/testdata/golden/worker_action/`; `UPDATE_GOLDEN=1 cargo test` schreibt sie neu. Die Werkzeugversion steht darin als `{version}`.
- Zeilen, die nur der Port ausgibt, werden vor dem Vergleich gefiltert: die `ATVPREP-PLAN`-Zeilen in `rust/crates/atvprep-core/tests/prepare_pipeline.rs`.
