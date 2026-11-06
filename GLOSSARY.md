# atvprep Auslieferung

Wie atvprep gebaut, signiert, auf dem Host `mm` und in den Servarr-Containern installiert, aktualisiert und zurückgenommen wird.

## Language

**Release**:
Eine Version von atvprep, bestehend aus Manifest, Signatur und je einem Asset pro Plattform. Wird lokal gebaut und signiert, nie in CI.
_Avoid_: Build, Paket, Wheel (Python-Altsystem)

**Asset**:
Die statisch gelinkte Binary eines Releases für genau eine Plattform, zum Beispiel `linux-x86_64`.
_Avoid_: Artefakt, Executable

**Manifest**:
Die Datei `release.json` mit Version und, je Plattform, URL, SHA-256 und Größe der Assets. Die Signatur gilt für ihre exakten Bytes.

**Vertrauensschlüssel**:
Der öffentliche OpenSSH-Ed25519-Schlüssel, gegen den Signaturen geprüft werden. Auf `mm` liegt genau eine root-eigene Kopie, die read-only in die Container eingebunden wird; eine Kopie aus Repo oder Download gilt nie als Vertrauensanker.
_Avoid_: Public Key, release.pub (das ist nur der Dateiname im Repo)

**Trust on first use**:
Die Erstinstallation übernimmt den Vertrauensschlüssel einmalig aus dem Release und zeigt seinen Fingerabdruck an; danach gilt nur noch die lokale Datei. Ein vorgegebener Fingerabdruck macht daraus eine strikte Prüfung.

**Hook-Register**:
Die root-eigene Datei `/etc/atvprep/hooks` mit je einer Zeile `<anwendung> <pfad>` je installiertem Hook. Ein Update aktualisiert alle eingetragenen Hooks.

**Plattform**:
Betriebssystem und Architektur eines Assets. Produktiv zählt nur `linux-x86_64`; alle anderen dienen Entwicklung und Erprobung.

**Multicall-Binary**:
Dieselbe Datei unter den Namen `atvprep`, `atvprep-manager` und `atvprep-hook`; der Aufrufname wählt den Einstiegspunkt.

**Manager**:
Der Dienst `atvprep manager api` auf `mm`. Er vereint API, Scanner, Executor und Wartung in einem Prozess.
_Avoid_: Worker (gibt es in Rust nicht als eigenen Dienst)

**Hook**:
Die Binary `atvprep-hook`, die Sonarr oder Radarr beim Import als Custom Script aufrufen. Sie liegt am stabilen Pfad `/config/atvprep-hook` im jeweiligen Container.

**Altsystem**:
Die Python-Implementierung samt Datenbank, Diensten und Runtime-Roots. Bleibt bis zu einer eigenen Löschentscheidung erhalten, ist aber nach dem Cutover gestoppt.

## Betrieb

**Releasefenster**:
Der kurze, bewusste Zeitraum, in dem Dienste und Imports gestoppt sind, die Datenbank gesichert und migriert und das neue Release aktiviert wird. Endet erst mit der Betreiberabnahme.
_Avoid_: Wartungsfenster, Deployment

**Gate**:
Der in der Datenbank gespeicherte Schalter, der bei geschlossenem Zustand neue Arbeit verhindert. Jede Änderung zählt eine Generation hoch; Öffnen verlangt die zuletzt gelesene Generation.

**Permit**:
Der Eintrag eines Prozesses, der bei offenem Gate gerade Arbeit ausführt. Offene Arbeit wird vor einem Releasefenster abgewartet (drain); Permits nachweislich beendeter Prozesse werden entfernt (reap).

**Cutover**:
Die einmalige Umstellung von `mm` vom Altsystem auf Rust mit frischer Datenbank. Erfordert eine dokumentierte Betreiberentscheidung.
_Avoid_: Migration (die Python-Datenbank wird nicht migriert)

**Rückfall**:
Die Rückkehr zu einer älteren Binary (Rollback) beziehungsweise nach dem Cutover zum Altsystem. Setzt weder das Datenbankschema noch Änderungen an Mediendateien zurück; Daten seit dem Fenster gehen verloren, wenn die Sicherung wiederhergestellt wird.
_Avoid_: Undo

**Abnahme**:
Die ausdrückliche Betreiberentscheidung, dass ein Release nach dem Releasefenster in Ordnung ist. Erst danach wird das Gate geöffnet.

**Zeitplan**:
Eine wiederkehrende Regel, die zu festen Zeitpunkten (Cron-Ausdruck, Serverzeit) eine Aktion auslöst: Prüfen, Reparieren, Konvertieren oder Scannen. Die Aktion eines Zeitplans ändert sich nach dem Anlegen nicht mehr. Ein Zeitplan, der Dateien ändert (Reparieren, Konvertieren), startet pausiert, braucht ein `max_files` von 1 bis 500 und wird bewusst angeschaltet; das Anschalten erzeugt die Signatur des Betreibers (HMAC, gebunden an Zeitplan-ID, Aktion, Bibliothek, Filter, `max_files` und die Einstellung `delete_original`). Jede Änderung dieser Parameter oder ein Pausieren entzieht die Freigabe; erneutes Anschalten signiert neu. Ein Auftrag aus einem Zeitplan darf Dateien nur mit gültiger, zu den aktuellen Parametern passender Signatur ändern, sonst wird er abgelehnt (siehe ADR 0006). Verpasste Läufe von Reparieren und Konvertieren werden nie nachgeholt. Für Prüfen und Scannen gilt: Nur der jüngste verpasste Slot wird nachgeholt, und nur innerhalb eines Fensters von 6 Stunden. Ein Slot vor dem Einschalten oder Ändern des Zeitplans läuft nie.
_Avoid_: Schedule, Cronjob, Prepare (die Aktion heißt Reparieren oder Konvertieren)

**Lauf**:
Ein einzelnes Auslösen eines Zeitplans, entweder zum geplanten Zeitpunkt (Slot) oder von Hand („Run now"). Erzeugt je nach Aktion einen Job oder einen Scan und wird mit Ergebnis im Verlauf des Zeitplans festgehalten.
_Avoid_: Ausführung, Run (nur in der Oberfläche)

**Slot**:
Der geplante Zeitpunkt eines Laufs, wie ihn der Cron-Ausdruck des Zeitplans ergibt.
