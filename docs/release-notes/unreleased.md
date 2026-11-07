# Nicht veröffentlicht

Hinweise für den Betreiber zum nächsten Release.

## Zeitpläne: Reparieren erfasst mehr Dateien

- Ein Zeitplan für Reparieren nimmt jetzt auch fertig geprüfte Dateien (`clean`) mit, bei denen sich noch eine Untertiteldatei einbetten lässt („Einbetten offen“). Bisher blieben sie außen vor.
- `max_files` bleibt die Bremse: Pro Lauf werden höchstens so viele Dateien angefasst, wie der Zeitplan erlaubt (Reparieren höchstens 12, Konvertieren höchstens 6).

## Zeitpläne müssen neu angeschaltet werden

- Die Freigabe „Re-Encode erlaubt“ gehört nun zur Signatur eines Zeitplans (ADR 0007). Migration 0009 pausiert deshalb alle eingeschalteten Zeitpläne für Reparieren und Konvertieren und vermerkt den Grund im Verlauf.
- Nach dem Update jeden dieser Zeitpläne prüfen und bewusst wieder anschalten; erst dann signiert er neu und läuft wieder.
- Prüfen- und Scan-Zeitpläne sind nicht betroffen.

## Re-Encode läuft jetzt wirklich

- Bisher startete der Manager jeden Auftrag im Kopiermodus; ein bestätigter Re-Encode oder ein signiertes „Re-Encode erlaubt“ führte nie zu einem Neu-Kodieren. Jetzt kodiert ein Item neu, dessen Freigabe das einschließt (Policy `copy-only-v2`, Feld `reencode` je Item, ADR 0007).
- Ein Re-Encode ersetzt die Datei und läuft nur, wenn die Einstellung `delete_original` an ist. Ohne sie bleibt die Datei unverändert und der Auftrag meldet „Re-Encode nötig, nicht ausgeführt“.
- Ein Re-Encode darf bis zu 4 Stunden dauern. Aufträge, die vor dem Update eingereiht wurden (`copy-only-v1`), werden abgewiesen und müssen neu eingereiht werden.

## Neue Datenbankmigrationen

- 0007: Kennzeichen „Re-Encode nötig“ je Datei, aus den bisherigen Prüfergebnissen abgeleitet.
- 0008: Spalte für die Meldung verwaister Teilüberarbeitungen am Scan.
- 0009: Freigabe „Re-Encode erlaubt“ je Zeitplan (Standard aus) und die Pause der betroffenen Zeitpläne.

Die Migrationen sind rein additiv; ein Rückfall auf die alte Binary setzt das Schema nicht zurück.
