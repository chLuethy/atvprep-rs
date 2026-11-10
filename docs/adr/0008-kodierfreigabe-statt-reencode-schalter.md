# Kodierfreigabe statt Re-Encode-Schalter

ADR 0007 kannte nur „Re-Encode ja/nein“ und meinte damit das Video. Audio, das kodiert werden muss, fiel durch dieses Raster: Eine Datei ohne Stereospur galt als normale Reparatur, lief im Kopiermodus und scheiterte dort mit „this fix would need to re-encode media“. Zugleich blockierte `delete_original` den Video-Re-Encode, obwohl ein Remux eine MP4 ohnehin an Ort und Stelle ersetzt und dabei kein Original übrig bleibt.

Deshalb gilt, in Manager, CLI und Hook gleich:

- **Drei Stufen:** Die Kodierfreigabe ist eine Obergrenze: Nur kopieren, Audio kodieren, Audio und Video kodieren. Re-Encode heißt nur noch das Neu-Kodieren des Videos.
- **`delete_original` ist entkoppelt:** Die Einstellung regelt nur noch, ob der alte Nicht-MP4-Container nach erfolgreichem Vorbereiten entfernt wird. Ob eine MP4 neu kodiert werden darf, entscheidet allein die Kodierfreigabe; ersetzt wird sie weiterhin erst nach bestandener Prüfung. Im Manager gilt das Löschen für jedes erfolgreiche Vorbereiten eines Nicht-MP4, auch ohne Kodierung, aber nicht rückwirkend für bereits konvertierte Dateien. Der Hook löscht nie; das übernehmen Sonarr und Radarr.
- **Eine Quelle:** Der Kodierbedarf (keiner, Audio, Video) wird vom selben Planer berechnet, der ausführt, und gegen ein globales Zielprofil (CRF, HEVC10, AV1 behalten, DTS-Fix). Das Frontend sagt nichts mehr selbst voraus.
- **Eine Aktion:** Reparieren und Konvertieren werden zu Vorbereiten, auch in Zeitplänen.
- **Standards:** Manueller Auftrag und CLI (`--encode`, ersetzt `--copy-only`) Audio kodieren, Hook fest Audio kodieren, Zeitplan Nur kopieren. Video muss man ausdrücklich freigeben, im manuellen Auftrag erzwingt das der Server. Zeitpläne signieren Kodierfreigabe und Zielprofil (siehe ADR 0006).
- **Bedarf über der Freigabe:** Die Datei bleibt komplett unverändert, und das Ergebnis lautet „Kodierung nicht freigegeben“, kein Fehler.

Verworfen: jede Kodierung „Re-Encode“ nennen (eine fehlende Stereospur bräuchte dann dieselbe Freigabe wie eine stundenlange HEVC-Kodierung) und bei fehlender Freigabe das Kopierbare trotzdem erledigen (die Datei würde zweimal umgeschrieben).

Die Folgen sind bewusst in Kauf genommen: Ein freigegebener Re-Encode ist auf einer MP4 nicht rückgängig zu machen. Eine MKV mit nötigem Video-Re-Encode wird ohne Freigabe nicht mehr kodiert, weder in der CLI ohne `--encode all` noch beim Import über den Hook, der dann mit Code 6 auf den Manager verweist. Bestehende eingeschaltete Zeitpläne werden erneut pausiert, weil sich ihre signierten Parameter ändern.
