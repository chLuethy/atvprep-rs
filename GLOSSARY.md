# atvprep Auslieferung

Wie atvprep gebaut, signiert, auf dem Host `mm` und in den Servarr-Containern installiert, aktualisiert und zurückgenommen wird, und wie es zur Laufzeit Arbeit und deren Status führt.

## Language

**Release**:
Eine Version von atvprep, bestehend aus Manifest, Signatur und je einem Asset pro Plattform. Wird lokal gebaut und signiert, in CI reproduzierbar nachgebaut, aber nie dort signiert. Veröffentlicht auf GitHub, danach auf den Spiegel übertragen.
_Avoid_: Build, Paket, Wheel (Python-Altsystem)

**Spiegel**:
Die Kopie von Repository und Releases auf Gitea, nur beim Release aktualisiert. Nimmt keine Arbeit an (keine Issues, PRs, CI) und dient als zweite Download-Quelle.
_Avoid_: Backup, Mirror (meint in Gitea die eingebaute Funktion, die nicht genutzt wird)

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

**Servarr-Image**:
Das Container-Image aus `docker/Dockerfile` für Sonarr, Radarr oder Whisparr v3: gepinnte Basis plus FFmpeg, Bento4, mediainfo und mkvtoolnix, aber ohne atvprep. Der Hook liegt nicht im Image, sondern im `/config`-Volume.
_Avoid_: Overlay-Image, Hook-Image

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
Eine wiederkehrende Regel, die zu festen Zeitpunkten (Cron-Ausdruck, Serverzeit) eine Aktion auslöst: Prüfen, Vorbereiten oder Scannen. Die Aktion eines Zeitplans ändert sich nach dem Anlegen nicht mehr. Ein Zeitplan, der Dateien ändert (Vorbereiten), startet pausiert, braucht ein `max_files` und wird bewusst angeschaltet; das Anschalten erzeugt die Signatur des Betreibers (HMAC, gebunden an Zeitplan-ID, Aktion, Mediathek, Filter, `max_files`, das Zielprofil, die Einstellung Original löschen und die Kodierfreigabe). Jede Änderung dieser Parameter oder ein Pausieren entzieht die Freigabe; erneutes Anschalten signiert neu. Ein Auftrag aus einem Zeitplan darf Dateien nur mit gültiger, zu den aktuellen Parametern passender Signatur ändern, sonst wird er abgelehnt (siehe ADR 0006 und 0008). Ein Zeitplan nimmt nur Dateien mit, deren Kodierbedarf innerhalb seiner Kodierfreigabe liegt (Standard: Nur kopieren). Verpasste Läufe von Vorbereiten werden nie nachgeholt. Für Prüfen und Scannen gilt: Nur der jüngste verpasste Slot wird nachgeholt, und nur innerhalb eines Fensters von 6 Stunden. Ein Slot vor dem Einschalten oder Ändern des Zeitplans läuft nie. Ein Slot, den der Manager nicht innerhalb des Fensters sah (2 Minuten bei Vorbereiten, 6 Stunden bei Prüfen und Scannen), wird genau einmal als „Verpasst“ (Slot, Grund) im Verlauf vermerkt, ohne Job; die Oberfläche rät, den Zeitplan anzupassen oder den Manager-Betrieb zu prüfen. Slots vor dem Einschalten, bei Pause oder geschlossenem Gate werden nicht vermerkt. Grenzen, die der Server beim Anlegen und Ändern prüft: Vorbereiten läuft höchstens alle 15 Minuten (kleinster Abstand zweier Slots, auch bei Listen wie `0,10 * * * *`); Prüfen und Scannen haben keinen Mindestabstand. `max_files` ist je Aktion begrenzt (Prüfen 500; Vorbereiten 12, mit Video in der Kodierfreigabe 6; Scannen hat keines). Ein bestehender Zeitplan über einer Grenze wird beim nächsten Speichern abgelehnt; ein eingeschalteter wird beim Auslösen pausiert und im Verlauf vermerkt (siehe ADR 0006).
_Avoid_: Schedule, Cronjob

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

**Auslöser**:
Woher ein Job stammt: von Hand (ein Betreiber hat ihn in der Oberfläche angestoßen), Zeitplan (aus einem Lauf eines bestimmten Zeitplans) oder Wiederholung (er wiederholt die fehlgeschlagenen Items eines früheren Jobs).
_Avoid_: Trigger, automatisch (kommt nur bei Altlasten vor)

**Scan-Status**:
Der Zustand eines Scan-Laufs (`ScanStatus`), getrennt vom Job-Lebenszyklus: `queued`, `claimed`, `running`, `succeeded`, `failed`, `cancelled`, `interrupted`. Terminal sind `succeeded`, `failed`, `cancelled` und `interrupted`.

**Terminal**:
Ein Status, aus dem kein weiterer Übergang mehr erwartet wird: die Arbeit ist auf die eine oder andere Weise beendet und ändert sich nicht mehr. Die Typen bieten dafür `is_terminal()` und, für SQL, `terminal_sql_list()`; Übergänge prüft `can_become()`.

## Dateien und Prüfergebnis

**Vorbereiten**:
Die eine Aktion, die eine Datei so weit bringt wie nötig, um dem Ziel zu genügen, begrenzt durch die Kodierfreigabe. „Reparieren“ und „Konvertieren“ sind als Namen dieser Aktion zu meiden; die Eigenschaft „zu konvertieren“ (die Datei liegt in einem Nicht-MP4-Container) bleibt erlaubt, weil sie die Datei beschreibt und keine Aktion benennt.
_Avoid_: Prepare, Reparieren, Konvertieren (als Aktion), Fix

**Re-Encode**:
Das Neu-Kodieren des Videos.
_Avoid_: Re-Encode für Audio, Video-Reencode (technischer Regelname)

**Audio-Kodierung**:
Das Neu-Kodieren von Audio, während das Video kopiert bleibt: Der Stereo-Downmix ergänzt eine AAC-Spur, der Audio-Transcode ersetzt eine nicht abspielbare Spur verlustbehaftet durch EAC3.

**Kodierbedarf**:
Was „Prüfen“ je Datei festhält: keiner, Audio, Video oder unbekannt, solange noch keine Prüfung gegen das aktuelle Zielprofil vorliegt. Er ist unabhängig vom Zustand (bereit, Warnung, Blocker) und stammt vom selben Planer, der später ausführt.
_Avoid_: Re-Encode nötig

**Änderungsart**:
Wie Vorbereiten eine Datei ändern würde, so wie „Prüfen“ es zusammen mit dem Kodierbedarf festhält: keine, Patch (nur Flags und Tags an Ort und Stelle), Remux (die Datei wird als MP4 neu geschrieben, alle Streams kopiert), Audio-Kodierung oder Re-Encode. Eine Kodierung geht dem Weg vor: Braucht ein Remux eine Audio-Kodierung, ist die Änderungsart Audio-Kodierung. Vorschau und Inspector zeigen sie, statt sie selbst vorherzusagen.
_Avoid_: „Remux“ für jede Änderung

**Kodierfreigabe**:
Die vom Betreiber gesetzte Obergrenze, was ein Auftrag oder Zeitplan kodieren darf: Nur kopieren, Audio kodieren, Audio und Video kodieren.
_Avoid_: Re-Encode erlaubt

**Zielprofil**:
Die globale Einstellung des Managers, gegen die Kodierbedarf und Vorbereiten rechnen: CRF, HEVC10, AV1 behalten, DTS-Fix.

**Kopiermodus**:
Die Kodierfreigabe „Nur kopieren“: Streams werden nur kopiert, nie neu kodiert.

**Original löschen**:
Die Einstellung `delete_original`: Nach erfolgreichem Vorbereiten im Manager wird der alte Nicht-MP4-Container entfernt, unabhängig von der Kodierfreigabe. Sie sagt nichts darüber, ob kodiert werden darf. Der Hook löscht nie; das übernehmen Sonarr und Radarr.

**Policy-Version**:
Die Kennung, unter der ein Job eingereiht wurde. Der Worker führt nur Jobs der aktuellen Version aus; ältere werden abgewiesen („neu einreihen“). Die aktuelle Version trägt je Item die Kodierfreigabe, die in die Signatur eingeht.

**Mediathek**:
Eine eingebundene Wurzel, unter der atvprep Dateien findet, zum Beispiel die Serien von Sonarr oder die Filme von Radarr. Ein Scan, ein Zeitplan oder ein Filter kann sich auf eine Mediathek oder auf alle beziehen.
_Avoid_: Bibliothek, Library, Root (als Begriff in der Oberfläche)

**Dateien**:
Die Ansicht aller bekannten Dateien über alle Mediatheken hinweg, mit Inspector und Vorschau zum Vorbereiten.
_Avoid_: Mediathek (meint eine Wurzel), Bibliothek, Library

**Untertitel einbettbar**:
Eine Eigenschaft einer Datei, bei der mindestens eine Untertiteldatei daneben liegt, die sich noch einbetten lässt, gleich welchen Zustand die Datei hat.
_Avoid_: Einbetten offen

**Einbettung offen**:
Ein Zustand: Die Prüfung ergab keine Verstöße, aber die Datei ist untertitel einbettbar. Sie ist damit noch nicht bereit. Zusammen mit Bereit, Warnung, Blocker, Nicht geprüft, Prüfung fehlgeschlagen und Fehlt teilt er die Dateien lückenlos auf.

**Bereit**:
Eine Datei, deren Prüfung keine Verstöße ergab und bei der nichts mehr eingebettet werden kann.
_Avoid_: Sauber, Clean (nur der interne Zustandsname)

**Eigenschaften**:
Merkmale einer Datei, die unabhängig vom Zustand gelten und sich überschneiden dürfen: zu konvertieren (Nicht-MP4-Container), Kodierbedarf, untertitel einbettbar, konvertiert (Original behalten). Gewählte Eigenschaften zeigen nur Dateien, die alle gewählten haben; gewählte Zustände dagegen jede Datei, die einen davon hat.

**Verwaiste Teilüberarbeitung**:
Eine halbfertige Arbeitsdatei, die nach einem Absturz oder Abbruch eines Vorbereitens neben der Quelle zurückbleibt. Ist nie ein Ergebnis und wird beim Start des Managers aufgeräumt. Was dabei stehen bleibt, weil es noch zu einem laufenden Vorgang gehören könnte, meldet der Scan; der Betreiber entfernt es dort nach Bestätigung mit Anzahl und Größe. Entfernt wird nur, wenn beides noch stimmt und die Datei nicht zu laufender Arbeit gehört. Jede Löschung steht im Verlauf.
_Avoid_: Temp-Datei, Leiche
