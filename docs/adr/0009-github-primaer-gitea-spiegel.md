# GitHub ist die Primärquelle; Gitea wird nur beim Release gespiegelt

Code, Issues, Pull Requests, CI und Releases liegen im öffentlichen Repository `chLuethy/atvprep-rs` auf GitHub. Gitea (`chl/atvprep-rs`) ist nur noch der Spiegel: Issues, Pull Requests und Actions sind dort abgeschaltet, und es gibt dort keine Workflows mehr. Nach einem erfolgreichen GitHub-Release pusht ein eigener Job `main` und den Tag ohne Force nach Gitea, lädt die bereits verifizierten Assets hoch und prüft sie dort erneut gegen das signierte Manifest. Gitea soll eine unabhängige zweite Quelle bleiben, aber keine zweite Arbeitsfläche, auf der Issues oder CI-Ergebnisse auseinanderlaufen.

## Erwogene Optionen

- **Gitea-Pull-Mirror von GitHub**: verworfen, weil er nur Code und Tags überträgt, keine Release-Assets, und zudem jeden Push spiegelt statt nur den veröffentlichten Stand.
- **Jeden Push auf `main` spiegeln**: verworfen. Gitea zeigt bewusst nur den zuletzt veröffentlichten Stand.
- **Gitea bleibt Download-Quelle, GitHub nur für Arbeit**: verworfen. Neue Releases sollen dort liegen, wo gearbeitet wird; Gitea bleibt über `ATVPREP_HOST` als Notfallquelle nutzbar.

## Folgen

- Standard-Download-Quelle von `scripts/install.sh` ist GitHub. Das Skript kennt beide Hosts und wählt API und Repo-Pfad je Host (`chLuethy/atvprep-rs` auf GitHub, `chl/atvprep-rs` sonst). Das signierte Manifest trägt GitHub-URLs; `install.sh` nutzt das Feld `url` nicht, daher gilt dasselbe Manifest auf dem Spiegel.
- Ein fehlgeschlagener Spiegel-Job nimmt das GitHub-Release nicht zurück und lässt sich einzeln wiederholen. Weicht Gitea-`main` ab, bricht der Push ab, statt den Spiegel zu überschreiben.
- Releases bis 0.5.8 liegen nur auf Gitea; das erste GitHub-Release ist 0.5.9.
- Die Gitea-Issues wurden mit Kommentaren nach GitHub übernommen und sind auf Gitea nicht mehr sichtbar. Alte Pull Requests leben nur in der Git-Historie weiter. Nummern in älteren Commit-Messages (`#n`) meinen Gitea und verlinken auf GitHub auf das falsche Issue; die Zuordnung steht in der ersten Zeile jedes übernommenen Issues.
- Der Downloader von `atvprep deploy` folgt höchstens drei Redirects, nur auf HTTPS ohne Zugangsdaten, weil GitHub Release-Downloads auf seinen Speicher-Host umleitet. Zuvor waren Redirects ganz abgeschaltet. Vertrauen entsteht weiterhin nur über Signatur, Größe und SHA-256, nicht über den Host.
