# Nicht veröffentlicht

Hinweise für den Betreiber zum nächsten Release.

## Zeitpläne: Reparieren erfasst mehr Dateien

- Ein Zeitplan für Reparieren nimmt jetzt auch fertig geprüfte Dateien (`clean`) mit, bei denen sich noch eine Untertiteldatei einbetten lässt („Einbetten offen“). Bisher blieben sie außen vor.
- `max_files` bleibt die Bremse: Pro Lauf werden höchstens so viele Dateien angefasst, wie der Zeitplan erlaubt (Reparieren höchstens 12, Konvertieren höchstens 6).

## Zeitpläne müssen neu angeschaltet werden

- Die Freigabe „Re-Encode erlaubt“ gehört nun zur Signatur eines Zeitplans (ADR 0007). Migration 0009 pausiert deshalb alle eingeschalteten Zeitpläne für Reparieren und Konvertieren und vermerkt den Grund im Verlauf.
- Nach dem Update jeden dieser Zeitpläne prüfen und bewusst wieder anschalten; erst dann signiert er neu und läuft wieder.
- Prüfen- und Scan-Zeitpläne sind nicht betroffen.

## Neue Datenbankmigrationen

- 0007: Kennzeichen „Re-Encode nötig“ je Datei, aus den bisherigen Prüfergebnissen abgeleitet.
- 0008: Spalte für die Meldung verwaister Teilüberarbeitungen am Scan.
- 0009: Freigabe „Re-Encode erlaubt“ je Zeitplan (Standard aus) und die Pause der betroffenen Zeitpläne.

Die Migrationen sind rein additiv; ein Rückfall auf die alte Binary setzt das Schema nicht zurück.
