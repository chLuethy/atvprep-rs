# Migration auf SolidJS 2 (#107–#119)

## Rahmen und Ausgangspunkt

- Ausgangscommit: `7a2178611943736b7393d16fd4ab2390ed26d3dc` auf `main`.
- Integrationszweig: `integration/solid-2`. Keine Übernahme eines inkompatiblen Zwischenstands.
- Clientseitige SPA; keine Änderungen an Backend, Datenverträgen, Freigaben oder Gestaltung.
- Paketmanager: npm mit `frontend/package-lock.json`; lokal Node `22.22.3`, npm `10.9.8`.
- Der Nutzer hat in dieser Sitzung **ausschließlich `@solidjs/testing-library@1.0.0-rc.2` als exakt gepinnte Vorabversion freigegeben**. Dies ist keine allgemeine Freigabe weiterer Vorabpakete.

## Kompatibilitätsprüfung am 19. November 2026

Registry: `https://registry.npmjs.org/`; geprüft mit `npm view <Paket>@<Version> … --json`.

| Bestandteil | Ausgangsversion | Zielkandidat | Beleg / notwendige Anpassung |
| --- | --- | --- | --- |
| Solid | 1.9.5 | 2.0.3 | Stabil; Node >=22.12.0. DOM-Subpaths entfallen; Effects und verzögerte Writes semantisch migrieren. |
| Web-Runtime | in Solid enthalten | 2.0.3 | Stabil; Peer `solid-js ^2.0.3`, eigene DOM- und JSX-Exports. |
| Vite-Plugin | vite-plugin-solid 2.11.6 | @solidjs/vite-plugin 3.0.1 | Stabil; Peers Solid/Web ^2, Vite ^8 oder ^9; Node >=22.12.0. Compiler-Abhängigkeit ^2.0.0. |
| Compiler | alter Babel-Compiler | @solidjs/compiler 2.0.2 | Stabiler Kandidat der Plugin-Abhängigkeit; tatsächliche Auflösung im Lockfile nach Umstellung prüfen. |
| Vite | 6.0.11 | 8.3.8 | Stabil; Node ^20.19.0 oder >=22.12.0. |
| Router | 0.15.3 | 2.0.0 | Stabil; Peers Solid/Web ^2.0.0; alte Router-/Route-/A-Komponenten entfallen. Statischer Router mit Routenobjekten. |
| TypeScript | 5.7.3 | 5.7.3 zunächst beibehalten | Kein Upgrade allein aufgrund einer höheren Registry-Version; Web-JSX-Konfiguration und Zieltypen gemeinsam prüfen. |
| Vitest | 2.1.9 | 5.1.1 | Stabil; Vite ^6.4, ^7 oder ^8; Node ^22.12, ^24 oder >=26; @types/node ^22 oder >=24. |
| Testing Library | 0.8.10 | 1.0.0-rc.2 | Einzige freigegebene Ausnahme; Peers Solid/Web >=2.0.0-0. 0.8.10 importiert entfernte APIs (`onMount`, `onError`, `solid-js/web`) und ist trotz breiter Peer-Range nicht geeignet. |
| Node lokal / CI | 22.22.3 / 22 | mindestens 22.12.0 | Lokaler Stand erfüllt Anforderung; CI-Untergrenze vor Runtime-Umstellung explizit festlegen. |
| Ark UI | 5.1.1 | entfernen | Keine Imports in Anwendung, Tests, E2E oder Konfiguration; ausschließlich in Manifest und Lockfile deklariert. Kein Ersatzpaket nötig. |

Die Matrix nennt zunächst geprüfte **Kandidaten**, nicht eine bereits erfolgreich integrierte Zielinstallation. Peer-Metadaten allein beweisen keine vollständige Laufzeitkompatibilität.

Die Kombination wurde zusätzlich in einem isolierten temporären npm-Projekt installiert, ohne Manifest, Lockfile oder `node_modules` der Anwendung zu ändern. Unter Node 22.22.3 bestanden TypeScript 5.7.3, ein jsdom-Komponententest mit Router 2, verzögertem Signal-Update (`flush`) und Async-Memo unter `Loading` sowie ein Produktionsbuild mit dem neuen Plugin. `npm ls` bestätigt eine einzige Solid-/Web-Runtime (je 2.0.3), Compiler 2.0.2, Vite 8.3.8 und Vitest 5.1.1. Im gesamten aufgelösten Abhängigkeitsbaum dieses Versuchs ist die freigegebene Testing Library die einzige Vorabversion. Das ist ein Kompatibilitäts-Smoke, **keine Abnahme der Anwendung**.

Relevante Exportgrenzen wurden gegen die veröffentlichten Pakete geprüft: Solid 2 stellt Core-Primitives bereit, aber keine Web-/JSX-Subpaths; Web 2 exportiert `render`, DOM-Typen und `jsx-runtime`; Router 2 stellt `createRouter` und `memoryHistory` statt der alten Komponenten bereit. Die Testauflösung muss Browser-Exports verwenden, nicht den unter Node ebenfalls angebotenen Serverpfad.

### Primärquellen

- [Versionsgebundener Migrationsguide](https://github.com/solidjs/solid/blob/68406c2077eff22c67f71db4a16bf7026ddceee1/documentation/solid-2.0/MIGRATION.md)
- [Versionsgebundenes Cheatsheet](https://github.com/solidjs/solid/blob/68406c2077eff22c67f71db4a16bf7026ddceee1/packages/solid/CHEATSHEET.md)
- [Versionsgebundener Agenten-Primer](https://github.com/solidjs/solid/blob/68406c2077eff22c67f71db4a16bf7026ddceee1/packages/solid/skills/solid-2.0/SKILL.md)
- [Offizielle Router-Migration](https://docs.solidjs.com/migration/from-solid-router.md)
- [Offizielle Testanleitung](https://docs.solidjs.com/guides/testing.md), verwendet weiterhin `@solidjs/testing-library@next`.

## Prüfstand vor der Runtime-Umstellung

- Typecheck: bestanden.
- Bestehende Frontend-Tests vor Ergänzungen: **44 Dateien, 746 Tests bestanden**; nach den vier Charakterisierungstests: **44 Dateien, 750 Tests bestanden**.
- Produktionsbuild mit Vite 6.0.11: bestanden.
- Zusätzliche Charakterisierung am bestehenden `api.request`-Seam: vertauschte Antwortreihenfolgen bei Datei- und Jobwechsel, einschließlich Inspektionen, Historie, Items und korrekt angefordertem Item-Protokoll. Gezielter Lauf einschließlich synchroner Mehrfachauslösung: **142 Tests bestanden**.
- Bestehende Inspector-/Jobtests prüfen außerdem Mehrfachübermittlung während einer laufenden Anfrage, Idempotency-Key-Wiederverwendung, Fokus und Sitzung abgelaufen. Zwei neue Tests lösen den Bestätigungsbutton zweimal im selben synchronen Ablauf aus; sie sichern die wichtige Grenze vor einem Solid-2-Commit zusätzlich ab.
- E2E-Baseline, separat mit `./scripts/run-e2e.sh`: **28 Chromium-Journeys bestanden** (14,7 Sekunden reine Testlaufzeit), einschließlich Entsperren, Dateien, Inspector, Spaltenansicht, Aktivität, Zeitplänen, Einstellungen, Navigation und Ansichtsgedächtnis.
- `./scripts/check-all.sh`: **fehlgeschlagen in `rust: deny`**. `cargo fmt`, Clippy und sämtliche **944 Rust-Tests** bestehen. Der Gate-Lauf erreicht wegen `set -e` die Frontend-/E2E-Schritte nicht; diese wurden daher zusätzlich separat ausgeführt.
- Konkreter vorbestehender Blocker der Gesamtprüfung: `chrono 0.4.45` ist laut **RUSTSEC-2026-0543**, `chrono-tz 0.10.4` laut **RUSTSEC-2026-0555** ungewartet. `cargo deny` meldet für beide keine sichere Upgrade-Version. Diese Meldungen stammen aus dem unveränderten Rust-Lockfile vor jeder Solid-2-Umstellung. Keine Advisory-Ausnahme, kein Rust-Upgrade und keine Änderung an Sicherheitsprüfungen wurden vorgenommen.
- Lokale Diagnoseprotokolle: `/tmp/atvprep-107-baseline.log` (Gesamtprüfung), `/tmp/atvprep-108-all-tests.log` (750 Frontend-Tests), `/tmp/atvprep-107-e2e-baseline.log` (28 Browser-Journeys). Diese temporären Dateien sind keine versionierten Artefakte.
- Bereits im Ausgangsstand sichtbare jsdom-Meldungen zur nicht implementierten Dokumentnavigation (Anker-/Downloadtests) sind getrennt von Solid-2-Diagnosen zu bewerten.

## Integrationsfolge

1. #108: Baseline, Kompatibilität und Charakterisierung abschließen.
2. #109–#112: gemeinsame Toolchain und vollständige Benutzerabläufe auf einem Integrationszweig kompatibel machen.
3. #113: grünes Integrationsgate einschließlich E2E.
4. #114–#118: Async-, Identitäts- und lokale Zustandslogik in verhaltensgeprüften Schritten vereinfachen.
5. #119: tatsächliche Versionen, entfernte und begründet verbleibende Helfer sowie Gesamtprüfung nachweisen.

## Historischer Lieferstand bei Übergabe am 20. November 2026, 05:08 CET

**Maßgeblicher Wiedereinstieg: [Übergabe mit Startauftrag, Ticketständen, Prüfbelegen und offenen Blockern](solid-2-handoff.md).** Die obige Matrix und der isolierte Smoke dokumentieren die historische Vorbereitung, nicht vollständig die aktuelle Installation.

#108 ist lokal vorbereitet. Die Plattform- und Workflow-Umstellung aus #109–#112 ist inzwischen weitgehend umgesetzt; Vereinfachungen aus #114, #115 und #118 wurden teilweise vorgezogen. Aktuell bestehen Typecheck, **750 Frontend-Tests in 44 Dateien**, Produktionsbuild und **28 Chromium-E2E-Journeys**. Die Anwendung verwendet tatsächlich Vitest **4.1.12**, nicht den ursprünglichen Kandidaten 5.1.1.

Die Migration ist trotzdem **nicht abgenommen**: #113–#119 sind nicht abgeschlossen. Das Gesamtgate bleibt durch die separat untersuchten Rust-Advisories blockiert. Zusätzlich wurden im aktuellen Lockfile zwei nicht freigegebene transitive Emnapi-Beta-Pakete im optionalen Rolldown-WASM-Zweig entdeckt; ihre Bereinigung ist offen. Produktionsauflösung, Diagnosen, verbleibende Identitäts-/Zustandslogik und ergänzte Abnahmetests sind ebenfalls weiter zu bearbeiten. Einzelheiten und nächste Schritte stehen ausschließlich in der Übergabe.

Dieser Absatz beschreibt die damalige Pause. Die anschließende Fortsetzung ist unten und in Abschnitt 0 der Übergabe dokumentiert. Keine Commits, Merges, Pushes, Releases oder Issue-Schließungen sind erfolgt.

## Fortsetzung am 20. November 2026, Prüfstand 05:45 CET

**Historischer Detailstand dieser Fortsetzung: Abschnitt 0 der [Übergabe](solid-2-handoff.md). Der neuere Stand folgt im nächsten Abschnitt und in Abschnitt 0a der Übergabe.**

- Abhängigkeiten bereinigt: Vite **8.1.7** → regulär kompatibles Rolldown **1.1.8** → stabile Emnapi-Versionen auch im optionalen WASM-Zweig. Vollständige Lockfile-Prüfung: ausschließlich die freigegebene Testing Library **1.0.0-rc.2** ist eine Vorabversion. Keine Overrides oder Peer-Ausnahmen.
- Tatsächliche Runtime: Solid, Web und Signals **2.0.3**, Router **2.0.0**; Plugin **3.0.1**, Compiler **2.0.2**, Vitest **4.1.12**. Normales `npm ci` mit npm 10.9.8 und `npm ls --all` bestanden.
- Erzwungene Development-Conditions entfernt; Modulgraph eines Produktionsbuilds auf Produktions-Runtimes geprüft. Tests behalten die Development-Diagnosen des Plugins. Beide CI-Jobs nutzen Node **22.22.3**, Manifest mindestens **22.12.0**.
- Manuelle Objektwiederverwendung und rekursiver Vergleich mit ignorierten IDs vollständig entfernt, einschließlich `stable.ts` und seiner fünf Implementierungstests. Explizite Schlüssel erhalten Datei-, Job- und Itemzeilen; aktuelle DTO-/Request-IDs werden nicht mehr durch alte IDs ersetzt.
- Dateischlüssel gelten pro Mediathek und Pfad. Jobschlüssel sind mangels garantierter natürlicher Eindeutigkeit nur geprüfte Kandidaten; mehrdeutige oder fehlende Schlüssel fallen auf Objektidentität zurück. Item-Positionen werden linear geprüft und bleiben auf die jeweilige Jobinstanz begrenzt.
- Neue Tests prüfen aktuelle API-IDs, Auswahl/Fokus, offene Logs, Ändern/Einfügen/Umsortieren/Entfernen, fehlende/doppelte Schlüssel einschließlich mehrdeutiger Zwischenantwort, Nachladefehler, wiederholtes Lesen sichtbarer Item-Seiten und synchrone Mehrfachauslösung.
- Settings behalten ihren bereits geeigneten Draft-Updater; kein mechanischer Store-Umbau. Wochentagsänderungen arbeiten mit dem vorherigen Draft. Zusätzliche synchrone Sperren schützen Speicherung und ausgewählte Mutationen vor verzögerter Signalübernahme.
- Aktuelle Belege: **764 Tests in 43 Dateien**, Typecheck und Produktionsbuild bestanden; **28 Chromium-Journeys** bestanden; `git diff --check` bestanden. Es wurden 19 Verhaltenstests ergänzt und fünf obsolete Helfertests entfernt.
- **Kein grünes Gesamtgate:** Zwei Gesamtprüfungen endeten durch Werkzeug-Zeitlimits während Rust-Tests (180 / 600 s). Format und Clippy bestanden bis dahin; kein neuer vollständiger Rust-Testabschluss. Separates `cargo deny check` bestätigt weiterhin die beiden unveränderten Chrono-Advisories.
- **Offene Integrationsreste:** Tests ohne Konsolenabfang zeigen weiterhin Solid-Reaktivitäts- und Checkbox-Diagnosen. Weitere Konkurrenz-/Formular-/Browsernachweise sowie vollständige Kriterienzuordnung fehlen. Generationen, Seitenanzahl und ausgewählte Synchronisierung bleiben deshalb bewusst bestehen.

Die Migration und #113–#119 sind **nicht pauschal abgenommen**. Keine Backend-/Schema-/API-Änderung, keine Advisory-Ausnahme und kein Deployment.

## Weitere Fortsetzung am 20. November 2026, Prüfstand 06:22 CET

Maßgeblich sind nun [Übergabe, Abschnitt 0a](solid-2-handoff.md) und die neue **[Kriterienmatrix #107–#119](solid-2-acceptance.md)**. Die älteren Prüfergebnisse oben bleiben als Verlauf erhalten.

- Erneutes normales `npm ci` und vollständiger Abhängigkeitsbaum bestanden. Versionen unverändert: Solid/Web/Signals 2.0.3, Router 2.0.0, Plugin 3.0.1, Compiler 2.0.2, Vite 8.1.7, Rolldown 1.1.8, Vitest 4.1.12. Ausschließlich die exakt freigegebene Testing Library bleibt eine Vorabversion.
- Alle zuvor beobachteten Solid-Development-Diagnosen im vollständigen unverdeckten Testlauf ursächlich behoben: Compute-/Apply-Snapshots, Spaltennavigation/-pfadrückmeldung, reaktive Übersicht und ausdrücklich serverbestätigte Zeitplan-Schalter. Keine Diagnoseunterdrückung.
- Refresh-Steuerung serialisiert Datei-Polls/Scan-Ereignisse sowie aktive/relative Job-Polls; vorgemerkte Invalidierung während Nachladen geht nicht verloren. Filterwechsel und überholte Zählerantworten sind abgesichert. Generationen und Seitenanzahl bleiben aus konkreten Benutzerverträgen nötig.
- Settings bewahren Änderungen während laufender Speicherung und verwerfen alte Auditantworten. Zeitplan-Modus-/Aktionswechsel verarbeiten den kumulierten Draft; verspätete Aktions- und Sprachvorschauen sind getestet. Synchrone Request-Sperren schützen weitere Mutationen und Nachladeaktionen.
- Verspätete Inspector-/Jobmutationen lösen nach Unmount keine Seiten-Callbacks/Refetches mehr aus. Die erste Zuordnung eines direkt geöffneten Jobs zur verspäteten Liste erhält seine laufende Detailaktion. Die Jobzuordnung ist jetzt eine reine Memo-Ableitung statt eines nachführenden Effects. Mehrdeutige Zwischenantworten dürfen trotzdem keinen offenen Zustand wiederbeleben.
- `itemsReady` entfernt; `itemPages` benennt den tatsächlichen Mehrseitenvertrag. Die unabhängige erste Item-Ladeanzeige sowie Nachlade-/Revalidierungssperren bleiben begründet erhalten.
- **782 Tests in 43 Dateien bestanden**, Typecheck und Produktionsbuild bestanden; **31 Chromium-Journeys bestanden**. Die Tests wurden mit `--disableConsoleIntercept` geprüft; keine Solid-`[CODE]`-Diagnosen, jsdom-Browsergrenzen weiterhin unverdeckt. Gegenüber dem Eintrittsstand 18 Verhaltenstests und drei Browser-Journeys ergänzt, vorhandene Mehrfachauslösungstests verschärft.
- Die neuen Browser-Journeys prüfen Dateifokus/DOM-Auswahl bei Reorder, offene Job-/Itemlogs bei kontrolliert wechselnden IDs samt Fokus und aktuellen Requests sowie Bearbeitung während langsamer Speicherung. Der Dateitest nutzt echte Serverantworten mit stabilen IDs; wechselnde Datei-IDs sind separat in UI-Integrationstests nachgewiesen.
- **Gesamtgate bleibt nicht grün.** Keine neue vollständige Projektprüfung in dieser Fortsetzung. Die separat untersuchten Rust-Advisories und der fehlende aktuelle Rust-Testabschluss bleiben offen, ohne Ausnahme oder Backend-Umbau. Zusätzliche Browserkombinationen aus Detail-/Nachladefehlern und Fokus sowie einzelne Mutations-Lifecycles bleiben in der Kriterienmatrix ausdrücklich als Nachweislücken sichtbar.

Alle Änderungen blieben zu diesem historischen Zeitpunkt uncommittet auf `integration/solid-2`; kein Push, Merge, Release oder GitHub-Schreibzugriff. Es wurde keine vollständige Migration oder Betreiberabnahme behauptet.

## Fortsetzung am 20. November 2026, Prüfstand 06:54 CET

Aktueller Wiedereinstieg: [Übergabe, Abschnitt 0b](solid-2-handoff.md) und [Kriterienmatrix](solid-2-acceptance.md).

- Neue rot/grün nachgewiesene Lifecycle-Korrektur für höhere Kodierfreigabe: Nach Verlassen der Jobdetails darf eine verspätete Antwort keine Seiten-Callbacks und Navigation mehr auslösen.
- Zwei zusätzliche Chromium-Journeys prüfen Inspektionsladefehler und Item-Nachladefehler mit verzögerter Wiederholung sowie erhaltener Bestätigung beziehungsweise offenem Protokoll und realem Browserfokus.
- Aktuell **783 Frontend-Tests in 43 Dateien**, Typecheck, Produktionsbuild und **33 Chromium-Journeys** bestanden. Normales `npm ci`, `npm ls --all`, Vorabversionsscan und Produktions-Modulgraph erneut bestanden; Plattformversionen unverändert.
- Die vollständige Projektprüfung erreichte diesmal ohne Werkzeug-Zeitlimit das Advisory-Gate: **Rust-Format, Clippy und alle 944 Tests in 71 Testprogrammen bestehen**. Weiterhin ausschließlich die bekannten beiden Chrono-Advisories; Bans, Lizenzen und Quellen grün. Keine Sicherheitsausnahme und kein Backend-Umbau.
- Der Nutzer hat issuebezogene Commits ausdrücklich beauftragt. Die vorhandenen verflochtenen Änderungen werden als zusammenhängende Modul-Batches auf `integration/solid-2` festgehalten, nach erneuter Gesamtprüfung vor jedem Commit; Befunde stehen in den Commit-Nachrichten und `/tmp/solid-107/commit-*.log`. Keine künstliche Behauptung unabhängiger Umsetzung oder Abnahme einzelner Tickets.
- #113/#119 bleiben durch das rote Gesamtgate blockiert. Keine Issue-Schließung, kein Push/Merge/Release und keine Betreiberabnahme.

## Fortsetzung am 20. November 2026, Prüfstand 13:12 CET

Aktueller Wiedereinstieg: [Übergabe, Abschnitt 0c](solid-2-handoff.md).

- #112: Drei zuerst rot reproduzierte Lifecycle-/Konkurrenzfehler behoben: verspätete Zeitplanaktion mit unnötigem Folgeread, weiterlaufende Freigabefakten-Kette nach Unmount und gestapelte langsame Scan-Polls.
- Ein Lebensdauersignal grenzt die Zeitplanansicht und ihre Freigabefakten ab. Polls starten keinen weiteren Read, solange die vorige Antwort aussteht. Keine pauschale Transportabbruch-Zusage; Servermutationen werden nicht zurückgenommen.
- Zehn zusätzliche Verhaltenstests decken Lauf/Pause/Löschen/Speichern/Freigabefakten, Scan-Lifecycle und Bereinigungserfolg/-konflikt nach Verlassen ab. Kein neuer produktiver Test-Seam und keine gemockten Solid-Primitiven.
- **793 Frontend-Tests in 43 Dateien**, Typecheck, Produktionsbuild und weiterhin **33 Chromium-Journeys** bestanden. Aktuelle Logs: `/tmp/solid-107/lifecycle-final-tests.log`, `lifecycle-e2e.log`; Gesamtprüfung vor Commit: `lifecycle-check-all.log`.
- Plattformversionen und Backend unverändert. #113/#119 bleiben ohne grünes Gesamtgate und unabhängigen Integrationsreview unvollständig. Kein Push/Merge/Release oder Issue-Abschluss.
