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
