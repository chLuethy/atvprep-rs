# Zeitpläne dürfen Dateien ändern

Bisher lehnte der Server Zeitpläne für Reparieren und Konvertieren ab, und die Freigabe-Prüfung wies jeden Auftrag mit `schedule_id` zurück: Dateien änderte nur, wer es von Hand und im Einzelfall freigab. Das war sicher, aber unbequem. Wer regelmäßig Neues einsortiert, muss jede Nacht selbst auf „Reparieren“ klicken.

Jetzt gibt es Zeitpläne für Reparieren und Konvertieren. Die Sicherheit kommt nicht mehr aus dem Verbot, sondern aus einer Signatur: Der Betreiber schaltet den Zeitplan bewusst an, und diese Bestätigung erzeugt eine HMAC-Signatur (derselbe Schlüssel und dieselbe Maschinerie wie bei der manuellen Freigabe, `approval.rs`). Sie bindet Zeitplan-ID, Aktion, Bibliothek, Filter, `max_files` und die Einstellung `delete_original` zum Zeitpunkt des Anschaltens. Die Signatur verfällt nicht nach Zeit, sondern mit dem Zeitplan:

- Jede Änderung dieser Parameter oder ein Pausieren entzieht die Freigabe (und pausiert den Zeitplan). Erneutes Anschalten signiert neu.
- Der Worker prüft die Signatur unmittelbar vor jeder Datei gegen die aktuellen Parameter des Zeitplans und die aktuelle Einstellung `delete_original`. Ein Auftrag ohne gültige Signatur, mit veränderten Parametern oder aus einem inzwischen pausierten oder gelöschten Zeitplan wird abgelehnt. Auch ein Auftrag, der vor einer Änderung eingereiht wurde, läuft danach nicht mehr.
- Die Datei-Identität (Gerät, Inode, ctime, Begleitdateien) wird wie bei der manuellen Freigabe gegen den Stand beim Einreihen geprüft.

Weitere Grenzen: Ein neuer Zeitplan dieser Art startet pausiert, `max_files` ist Pflicht (1 bis 500), das Gate muss offen sein. Verpasste Läufe werden **nie** nachgeholt: Ein Lauf findet nur in der Minute seines Slots statt, nicht im 6-Stunden-Fenster von Prüfen und Scannen. Wer den Server über eine Nachtzeit hinweg stoppt, bekommt am Morgen keine Änderungen auf einmal.

Der Preis ist Komfort gegen Vorsicht: Der Betreiber gibt eine Absicht („jede Nacht bis zu 25 Dateien“) im Voraus, nicht jede einzelne Änderung. Das ist schwer rückgängig zu machen, weil geänderte Mediendateien nicht zurückkehren, und kein Rückfall (siehe ADR 0001) setzt sie zurück. Deshalb gilt die Freigabe nur für genau die signierten Parameter, und die Bestätigung beim Anschalten (Ticket 30) nennt die aktuelle Trefferzahl und, falls `delete_original` gilt, das Löschen der Originale.

Nicht gelöst: Der Server prüft beim Einreihen nur, dass eine Signatur vorhanden ist; das Geheimnis hat nur der Worker-Prüfpfad. Ein manipulierter Datenbankeintrag kann also einen Auftrag einreihen, aber nie ausführen lassen.
