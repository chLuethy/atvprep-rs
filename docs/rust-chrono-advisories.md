# Separater Befund: Chrono-Wartungsstatus

Geprüft am 19. November 2026 vor der Solid-2-Migration, Ausgangscommit `7a2178611943736b7393d16fd4ab2390ed26d3dc`.

## Reproduktion und Ursache

`cd rust && cargo deny check advisories` schlägt reproduzierbar mit zwei unmaintained-Meldungen fehl:

- [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543): `chrono 0.4.45`.
- [RUSTSEC-2026-0555](https://rustsec.org/advisories/RUSTSEC-2026-0555): `chrono-tz 0.10.4`.

Beide aktuellen Einträge in der [offiziellen Advisory-Datenbank](https://github.com/RustSec/advisory-db) sind vom 14. September 2026, nicht zurückgezogen und haben eine leere Liste reparierter Versionen. Die Ursache ist also weder ein veralteter Cache noch ein durch normales Versionsupdate behebbarer Fehler. Der Maintainer bestätigt die Wartungsaufgabe ausdrücklich am 17. November im [Chrono-Issue 1768](https://github.com/chronotope/chrono/issues/1768); maßgeblich ist der dortige Kommentar des Maintainers, nicht der geschlossene Issue-Status allein.

Die Meldungen benennen Wartungsrisiken, keine konkret nachgewiesene ausnutzbare Sicherheitslücke in atvprep. `cargo deny` behandelt sie trotzdem richtigerweise als fehlgeschlagene Prüfung.

## Betroffenheit

`cargo tree -i chrono` und `cargo tree -i chrono-tz` belegen aktive direkte Abhängigkeiten: API, Manager und Fixtures verwenden Chrono; der Manager zusätzlich chrono-tz. Eine bloße Entfernung eines ungenutzten transitiven Pakets reicht nicht. Die Quellcodesuche findet rund 89 zeitbezogene Treffer; die Zahl ist nur eine Orientierung, keine Aufwandsschätzung.

Eine Ablösung, etwa durch Jiff oder time, wäre ein eigenständiger Backend-Eingriff. Insbesondere müssen Kalender, lokale Zeitzone, Sommerzeit-Lücken und doppelte Ortszeiten, Cron-Slots, Datenbank-/API-Zeitformate und Golden-Parität erhalten bleiben. Ein mechanischer Austausch wäre nicht ausreichend.

## Konsequenz für #107–#119

Keine Änderung an `rust/deny.toml`, keine Ausnahme und kein Backend-Umbau im Frontend-Integrationszweig. Die Meldungen bleiben sichtbar. Der Nutzer hat nach der separaten Untersuchung die Fortsetzung der Frontend-Arbeit ohne weitere Rückfrage beauftragt. Die bekannte rote Gesamtprüfung wird getrennt von Frontend-Regressionen berichtet; eine vollständig grüne Integrationsabnahme darf damit nicht behauptet werden.
