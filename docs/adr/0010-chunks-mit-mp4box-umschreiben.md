# Chunks der Zieldatei mit MP4Box umschreiben

ffmpegs mov-Muxer bildet Chunks von ein bis zwei Samples und lässt sich nicht auf eine Chunk-Dauer einstellen. Auf Apple TV stockten solche Dateien; `MP4Box -inter 500` erzeugt Chunks von rund 0,5 s Medienzeit und lief dort problemlos. Deshalb schliesst jedes Vorbereiten, das ffmpeg die Zieldatei schreiben lässt, mit einem MP4Box-Lauf ab (ffmpeg, MP4Box, moov-Patch). GPAC (`MP4Box`) kommt dafür ins Servarr-Image. „Prüfen“ hält die Chunk-Länge fest (mittlere Chunk-Dauer von Audio und Video unter 0,25 s, nur aus dem moov); eine Datei, bei der sie nicht passt, ist nie ein reiner Patch, sondern ein Remux.

- **Kein Zeitplan-Massenumschreiben:** Ein Zeitplan nimmt Dateien nicht mit, deren einziger Grund die Chunk-Länge ist. Sie werden von Hand (manueller Vorbereiten-Auftrag) umgeschrieben; die signierte Freigabe nach ADR 0006 war nie auf ein Umschreiben der ganzen Mediathek angelegt.
- **Bestehende Ergebnisse veralten:** Eine Migration lässt die Prüfergebnisse neu berechnen (wie 0013).
- **Image und Hook gehören zusammen:** Das Image baut seine Werkzeuge aus dem Quelltext mit gepinnten Hashes (musl); MP4Box muss ebenso gebaut werden, und das Image muss vor oder mit dem Hook aktualisiert werden.

Verworfen: ein eigener Umsortierer in Rust im `mp4`-Modul. Er käme ohne neue Abhängigkeit aus, müsste aber `mdat` neu schreiben und `stsc`/`stco` neu bauen; das ist deutlich mehr Code und Risiko als ein erprobtes Werkzeug. Ebenfalls verworfen: ffmpeg-Optionen (`-max_interleave_delta` und Verwandte), weil sie den Interleave zwischen den Spuren steuern, nicht die Chunk-Dauer.
