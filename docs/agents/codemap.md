# Wegweiser durch den Code

Einstiegspunkte nach Modul und Konzept, nicht nach Funktion. Suche den Namen im Modul selbst. Alle Pfade gelten vom Repo-Wurzelverzeichnis aus.

## Frontend (`frontend/src`)

- **Routen und Ansichtsgedächtnis.** `App.tsx` hängt die Routen ein. `Library` (Dateien) und `Activity` (Aktivität) sind Routen-Wrapper: eine Route für Liste und geöffnetes Element, der Filter steht in der Adresse. `lib/viewMemory.ts` merkt den zuletzt gezeigten Ansichtsstand pro Seite und ist der einzige Zugriff auf den Browser-Speicher dafür.
- **Dateien und Spaltenansicht.** `features/files/`: `LibraryPage.tsx` (Liste), `ColumnsView.tsx` mit `browse.ts` (Ordner nachladen) und `columnPath.ts` (offener Pfad, über `viewMemory`), `filters.ts`, `loaders.ts`, `Inspector.tsx`. `preparePlan.ts` und `fixPlan.ts` stellen Planer-Ergebnisse des Servers dar.
- **Aktivität und Protokollansicht.** `features/activity/`: `ActivityPage.tsx` (Jobliste), `JobDetail.tsx` (Job und Items), `LogViewer.tsx` (Item-Protokoll), `filters.ts`, `loaders.ts`, `model.ts`.
- **Texte.** `lib/i18n.ts` und `locales/de*.ts`.

## API (`rust/crates/atvprep-api/src`)

- **Lesepfade.** `read_files.rs` (Dateien, Ordner-Browse, Detail, Inspektionen) und `read_jobs.rs` (Jobs, Warteschlange, Items, Item-Protokoll), dazu `job_filter.rs` und `filter_params.rs` für die Abfrageparameter.
- **Fehlerkonvention.** Fehler gehen als `ProblemDetail` aus `errors.rs` hinaus. Ein ungültiger Parameter oder Body ist 422 (`UNPROCESSABLE_ENTITY`, Code `invalid_request`).

## Kern (`rust/crates/atvprep-core/src`)

- **Planer.** `planner.rs`: `build_plan` entscheidet pro Datei, was geschieht, und erzeugt die ffmpeg-Argumente (`video_args`, `audio_args`, `subtitle_args`).
- **Audio-Analyse.** `analyzers.rs` (`analyze_audio`, dazu die Video- und Untertitel-Analyse) bewertet die Spuren, bevor der Planer plant.
- **FFmpeg-Lauf.** `prepare/ffmpeg.rs` startet das geplante Kommando und liest dessen Fortschritt; `prepare/pipeline.rs` ordnet die Stufen, `stages.rs` definiert die Maschinenzeilen auf stdout.

## Manager (`rust/crates/atvprep-manager/src`)

- **Worker-Ausführung.** `worker/execution.rs` ist die Hülle (Lease, Fencing, Permit, Zustandsübergänge des Items). `worker/action.rs` hält je Aktion (Vorbereiten, Prüfen, Analysieren) Argumente, Deutung des Prozessergebnisses und Protokollzeilen; Seams sind `ProcessAdapter` (echter Kindprozess oder festes Ergebnis), `Sink` und `Shell`. `worker/process.rs` startet die Prozesse.
- **Item-Protokoll.** `worker/logs.rs` (`ItemLog`, begrenzt, Schrittzeilen bleiben ganz), `worker/report_log.rs` und `worker/prepare_log.rs` (Schrittzeilen aus dem, was das Werkzeug meldet).
- **Dateizustand.** `file_state.rs` entscheidet den Zustand einer Datei (Bereit, Einbettung offen, Warnung, …) an einer Stelle: SQL-Ausdruck `state_sql`, Rangfolge `RANKED`/`rank_sql`, Rust-Gegenstück `state_of`, dazu `is_preparable`. Filter, Sortierung, Spaltenansicht und Übersicht der API nutzen ihn; die Lese-API liefert `state` und `preparable` je Datei. Neuer Zustand: nur dort (Tabellentest) und in `frontend/src/features/files/filters.ts`.
- **Migrationen.** `schema.rs` führt `migrations/000N_name.sql` der Reihe nach aus; `PRAGMA user_version` ist die Version. Neu: Datei anlegen, in `STAGES` anhängen, `VERSION` erhöhen.

## Golden- und Oracle-Daten (`rust/testdata/golden`)

Die JSON-Dateien halten das Verhalten des Python-Altsystems fest; Rust-Tests laden sie mit `load_golden` und müssen sie treffen. Eine gewollte Abweichung wird im Test markiert, die Datei bleibt unverändert:

- Ein Fall wird namentlich ausgenommen und eigens geprüft: `RESAMPLED_SINCE_83` in `rust/crates/atvprep-core/src/planner.rs` (der Oracle kopierte die Spur mit 44,1 kHz, seit #83 wird sie mit 48 kHz kodiert).
- Zeilen, die nur der Port ausgibt, werden vor dem Vergleich gefiltert: die `ATVPREP-PLAN`-Zeilen in `rust/crates/atvprep-core/tests/prepare_pipeline.rs`.
