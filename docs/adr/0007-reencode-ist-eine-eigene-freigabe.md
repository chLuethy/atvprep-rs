# Re-Encode ist eine eigene Freigabe

Ein Re-Encode ändert eine Datei tiefgreifender und dauert länger als das Reparieren der Metadaten. Bisher entschied ein Nebeneffekt darüber, ob ein Zeitplan Dateien mit nötigem Re-Encode mitnimmt: Nur wenn die Einstellung `delete_original` an war, wurden sie berücksichtigt. Ein Betreiber konnte also nicht ausdrücklich „ja, neu kodieren“ sagen, und wer `delete_original` ansah, entschied unbemerkt mit.

Jetzt ist „Re-Encode nötig“ eine Eigenschaft der Datei, die „Prüfen“ festhält (siehe Glossar), und das Neu-Kodieren braucht in jedem Fall eine ausdrückliche Freigabe:

- **Manuell:** Die Vorschau nennt, wie viele der gewählten Dateien neu kodiert werden müssen. Der Betreiber löst entweder „Mit Re-Encode“ (bestätigt, gilt nur für diesen Auftrag) oder „Ohne Re-Encode“ aus. Der Server lehnt einen Auftrag mit Re-Encode-Dateien ohne Bestätigung ab; die Oberfläche allein reicht nicht.
- **Zeitplan:** Der Zeitplan hat ein eigenes Feld „Re-Encode erlaubt“ (Standard aus). Es gehört zu den signierten Parametern (siehe ADR 0006): Änderung oder Pausieren entzieht die Freigabe, Anschalten signiert neu. Die Kopplung an `delete_original` entfällt; `delete_original` bleibt Teil der Signatur, steuert aber nicht mehr, welche Dateien in Frage kommen.

Bestehende Zeitpläne bekommen „Re-Encode erlaubt = aus“. Weil sich ihre signierten Parameter ändern, verlieren eingeschaltete Zeitpläne bei der Umstellung die Freigabe und werden pausiert; der Verlauf nennt den Grund. Das ist unbequem, aber die Alternative wäre, eine Signatur über Parameter gelten zu lassen, die es beim Signieren nicht gab.

Verworfen: die Kopplung an `delete_original` beibehalten. Sie wäre bequemer, aber zwei unabhängige Entscheidungen („Original löschen“ und „neu kodieren“) hingen an einem Schalter.
