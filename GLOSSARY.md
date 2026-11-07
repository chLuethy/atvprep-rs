# atvprep Auslieferung

Wie atvprep gebaut, signiert, auf dem Host `mm` und in den Servarr-Containern installiert, aktualisiert und zurückgenommen wird, und wie es zur Laufzeit Arbeit und deren Status führt.

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
Eine wiederkehrende Regel, die zu festen Zeitpunkten (Cron-Ausdruck, Serverzeit) eine Aktion auslöst: Prüfen, Reparieren, Konvertieren oder Scannen. Die Aktion eines Zeitplans ändert sich nach dem Anlegen nicht mehr. Ein Zeitplan, der Dateien ändert (Reparieren, Konvertieren), startet pausiert, braucht ein `max_files` und wird bewusst angeschaltet; das Anschalten erzeugt die Signatur des Betreibers (HMAC, gebunden an Zeitplan-ID, Aktion, Bibliothek, Filter, `max_files`, die Einstellung `delete_original` und die Freigabe „Re-Encode erlaubt“). Jede Änderung dieser Parameter oder ein Pausieren entzieht die Freigabe; erneutes Anschalten signiert neu. Ein Auftrag aus einem Zeitplan darf Dateien nur mit gültiger, zu den aktuellen Parametern passender Signatur ändern, sonst wird er abgelehnt (siehe ADR 0006 und 0007). Ein Zeitplan nimmt Dateien mit „Re-Encode nötig“ nur mit, wenn „Re-Encode erlaubt“ gesetzt ist (Standard aus). Verpasste Läufe von Reparieren und Konvertieren werden nie nachgeholt. Für Prüfen und Scannen gilt: Nur der jüngste verpasste Slot wird nachgeholt, und nur innerhalb eines Fensters von 6 Stunden. Ein Slot vor dem Einschalten oder Ändern des Zeitplans läuft nie. Ein Slot, den der Manager nicht innerhalb des Fensters sah (2 Minuten bei Reparieren und Konvertieren, 6 Stunden bei Prüfen und Scannen), wird genau einmal als „Verpasst“ (Slot, Grund) im Verlauf vermerkt, ohne Job; die Oberfläche rät, den Zeitplan anzupassen oder den Manager-Betrieb zu prüfen. Slots vor dem Einschalten, bei Pause oder geschlossenem Gate werden nicht vermerkt. Grenzen, die der Server beim Anlegen und Ändern prüft: Reparieren und Konvertieren laufen höchstens alle 15 Minuten (kleinster Abstand zweier Slots, auch bei Listen wie `0,10 * * * *`); Prüfen und Scannen haben keinen Mindestabstand. `max_files` ist je Aktion begrenzt (Prüfen 500, Reparieren 12, Konvertieren 6; Scannen hat keines). Ein bestehender Zeitplan über einer Grenze wird beim nächsten Speichern abgelehnt; ein eingeschalteter wird beim Auslösen pausiert und im Verlauf vermerkt (siehe ADR 0006).
_Avoid_: Schedule, Cronjob, Prepare (die Aktion heißt Reparieren oder Konvertieren)

**Lauf**:
Ein einzelnes Auslösen eines Zeitplans, entweder zum geplanten Zeitpunkt (Slot) oder von Hand („Run now"). Erzeugt je nach Aktion einen Job oder einen Scan und wird mit Ergebnis im Verlauf des Zeitplans festgehalten.
_Avoid_: Ausführung, Run (nur in der Oberfläche)

**Slot**:
Der geplante Zeitpunkt eines Laufs, wie ihn der Cron-Ausdruck des Zeitplans ergibt.

## Arbeit und Status

**Job-Status**:
Der Zustand eines Jobs (`JobStatus`), abgeleitet aus seinen Item-Status: `queued`, `running`, `cancel_requested`, `succeeded`, `partial_success`, `failed`, `cancelled`. Terminal sind `succeeded`, `partial_success`, `failed` und `cancelled`.
_Avoid_: Parent-Status

**Item-Status**:
Der Zustand eines Items eines Jobs (`ItemStatus`): `queued`, `claimed`, `running`, `cancel_requested`, `cancelling`, `succeeded`, `failed`, `stale_input`, `cancelled`, `interrupted`. Terminal sind `succeeded`, `failed`, `stale_input`, `cancelled` und `interrupted`.
_Avoid_: Child-Status

**Scan-Status**:
Der Zustand eines Scan-Laufs (`ScanStatus`), getrennt vom Job-Lebenszyklus: `queued`, `claimed`, `running`, `succeeded`, `failed`, `cancelled`, `interrupted`. Terminal sind `succeeded`, `failed`, `cancelled` und `interrupted`.

**Terminal**:
Ein Status, aus dem kein weiterer Übergang mehr erwartet wird: die Arbeit ist auf die eine oder andere Weise beendet und ändert sich nicht mehr. Die Typen bieten dafür `is_terminal()` und, für SQL, `terminal_sql_list()`; Übergänge prüft `can_become()`.

## Dateien und Prüfergebnis

**Re-Encode nötig**:
Eine Eigenschaft einer Datei, die „Prüfen" festhält: Das Video muss neu kodiert werden, damit die Datei den Anforderungen genügt. Sie ist unabhängig vom Zustand (bereit, Warnung, Blocker) und wird mit dem Prüfergebnis gespeichert, nicht aus Begründungstexten erraten.
_Avoid_: Video-Reencode (technischer Regelname)

**Einbetten offen**:
Eine Eigenschaft einer Datei, bei der mindestens eine Untertiteldatei daneben liegt, die sich noch einbetten lässt. Ändert den gespeicherten Zustand nicht, aber die Datei gilt dann nicht als bereit.

**Bereit**:
Eine Datei, deren Prüfung keine Verstöße ergab und bei der nichts mehr eingebettet werden kann.
_Avoid_: Sauber, Clean (nur der interne Zustandsname)

**Eigenschaften**:
Merkmale einer Datei, die unabhängig vom Zustand gelten und sich überschneiden dürfen: zu konvertieren, Re-Encode nötig, Einbetten offen, konvertiert (Original behalten). In der Bibliothek zeigen gewählte Eigenschaften nur Dateien, die alle gewählten haben.

**Verwaiste Teilüberarbeitung**:
Eine halbfertige Arbeitsdatei (`.tmp_fixmeta_…`), die nach einem Absturz oder Abbruch neben der Quelle zurückbleibt. Ist nie ein Ergebnis und wird beim Start des Managers aufgeräumt. Was der Start-Aufräumlauf stehen lässt (jüngere Dateien, lebende PID), meldet der Scan: Anzahl, Größe und eine kurze Liste im Scan-Ergebnis, ein Hinweis in der Übersicht nur bei Treffern. Der Betreiber entfernt sie dort nach Bestätigung mit Anzahl und Größe; der Server löscht nur, wenn beides noch stimmt, nur Dateien mit dem exakten Namensmuster innerhalb einer Bibliothek und nie im Verzeichnis eines Items mit Status „claimed“ oder „running“ (nur dasselbe Verzeichnis, keine Unterverzeichnisse). Jede Löschung steht als Ereignis `orphan.removed` im Verlauf, die Sammelmeldung `orphan.cleanup` in der letzten Aktivität.
_Avoid_: Temp-Datei, Leiche
