# Release-Assets

Ein Release wird zweimal gebaut: lokal für das signierte Manifest und in GitHub Actions aus dem getaggten Quelltext. Beide Builds müssen bytegleich sein. Keine Binary wird eingecheckt. Die vollständige [Release-Checkliste](../docs/releasing.md) beschreibt die Vorbereitung und den Betrieb.

Dateien in diesem Verzeichnis:

- `release.json`: Manifest mit Workspace-Version und genau einem Eintrag pro Plattform. Jeder Eintrag enthält `platform`, `url` (HTTPS), `sha256` und `size`.
- `release.json.sig`: OpenSSH-Ed25519-Signatur der exakten Manifestbytes im Namensraum `atvprep-release`.
- `release.pub`: öffentlicher Schlüssel zur Signatur. Die Datei allein ist kein Vertrauensanker; GitHub prüft gegen den unabhängig hinterlegten Schlüssel `RELEASE_TRUSTED_KEY` (siehe ADR 0003).
- `atvprep-linux-x86_64` und `atvprep-linux-aarch64`: nicht eingecheckte Binaries. Die Prüfjobs laden ihre eigenen Builds zur Prüfung hierher.

## Lokaler Build und Signatur

Workspace-Version in `rust/Cargo.toml` erhöhen, die Änderungen prüfen und committen. Danach aus einer sauberen Arbeitskopie bauen:

```bash
./rust/release/build.sh /private/tmp/atvprep-release
./scripts/make-manifest.sh /private/tmp/atvprep-release
ssh-keygen -Y sign -f /pfad/zum/privaten-schluessel -n atvprep-release release/release.json
```

Der private Schlüssel bleibt lokal. Die URLs zeigen standardmässig auf das GitHub-Release (`RELEASE_BASE_URL` überschreibt sie). `scripts/install.sh` lädt vom gewählten Host und prüft die Bytes gegen das Manifest. Dasselbe signierte Manifest gilt deshalb auch auf dem Gitea-Spiegel.

Nur `release/release.json`, `release/release.json.sig` und `release/release.pub` committen, nicht die Binaries. Diese drei Dateien sind keine Eingaben für den Build. Den Commit als `v<workspace-version>` taggen und den Tag pushen.

## GitHub und Gitea

1. `build` prüft die Version und baut x86_64 und ARM64 auf je einem nativen Runner parallel mit demselben Dockerfile, das lokal verwendet wird.
2. `verify` prüft beide Assets gegen das signierte Manifest, kontrolliert ELF-Architektur, statische Linkung und CLI und testet die signierte Installation auf beiden Architekturen nativ. Ein manueller Workflow-Lauf ohne `mirror_version` endet hier.
3. `publish` prüft die konkret hochzuladenden Bytes erneut. Nur beim Tag-Push legt der Job einen Entwurf an, lädt die fünf Dateien hoch, lädt sie zum Vergleich wieder herunter und veröffentlicht erst nach erfolgreichem Vergleich. Ein fehlgeschlagener Upload kann einen unveröffentlichten Entwurf hinterlassen, den ein erneuter Lauf ersetzt.
4. `mirror` lädt das veröffentlichte GitHub-Release, prüft es gegen das signierte Manifest, pusht den Tag und dessen Commit als Gitea-`main` (nie mit Force), legt dasselbe Release auf Gitea an und vergleicht die öffentlich herunterladbaren Bytes. Schlägt nur der Spiegel fehl, kann man ihn mit `mirror_version` einzeln erneut starten.

Stimmt ein Build nicht mit den signierten Hashes überein, wird nichts veröffentlicht. In diesem Fall die Ursache klären und niemals ersatzweise die CI-Ausgabe signieren.
