# Release-Checkliste

Ein Release wird lokal gebaut und signiert; die GitHub-Action baut dieselben Bytes nach, veröffentlicht nur, wenn sie zum signierten Manifest passen, und überträgt das Release danach auf den Spiegel auf Gitea (ADR 0002, ADR 0005, ADR 0009). Details zu den Dateien: [release/README.md](../release/README.md). Die Beispiele verwenden `0.5.6`.

Der private Schlüssel `~/.ssh/atvprep-release-ed25519` verlässt den Rechner nie.

## Einmalig

- [ ] Actions-Secret `RELEASE_TRUSTED_KEY` auf GitHub enthält den Inhalt von `~/.ssh/atvprep-release-ed25519.pub`.
- [ ] Actions-Secret `GITEA_TOKEN` (Gitea-Token mit Lesen und Schreiben auf Repositories) und die Variablen `GITEA_URL` und `GITEA_REPO` sind auf GitHub gesetzt.
- [ ] Fingerabdruck des Schlüssels (`ssh-keygen -lf ~/.ssh/atvprep-release-ed25519.pub`) ausserhalb von GitHub und Gitea notiert (ADR 0003).
- [ ] Docker läuft, `docker buildx` ist verfügbar.

## Pro Release

1. [ ] **Version erhöhen**: `version` unter `[workspace.package]` in `rust/Cargo.toml` auf `0.5.6` setzen.
2. [ ] **Prüfen**: `./rust/check.sh`, dann in `frontend/` `npm ci && npm run typecheck && npm test && npm run build`.
3. [ ] **Committen** (Version und Änderungen), noch nicht taggen.
4. [ ] **Lokal bauen** (Ausgabe ausserhalb des Repos, der arm64-Build ist langsam):

   ```bash
   ./rust/release/build.sh /private/tmp/atvprep-release
   ```

5. [ ] **Manifest erzeugen**:

   ```bash
   ./scripts/make-manifest.sh /private/tmp/atvprep-release
   ```

6. [ ] **Signieren** (Namespace exakt `atvprep-release`):

   ```bash
   ssh-keygen -Y sign -f ~/.ssh/atvprep-release-ed25519 -n atvprep-release release/release.json
   ```

   Danach `release/release.json` nicht mehr verändern; die Signatur gilt für ihre exakten Bytes.
7. [ ] **Signatur gegen den Schlüssel prüfen**, bevor etwas gepusht wird:

   ```bash
   printf 'atvprep %s\n' "$(cat ~/.ssh/atvprep-release-ed25519.pub)" > /private/tmp/allowed-signers
   ssh-keygen -Y verify -f /private/tmp/allowed-signers -I atvprep -n atvprep-release -s release/release.json.sig < release/release.json
   cmp ~/.ssh/atvprep-release-ed25519.pub release/release.pub
   ```

8. [ ] **Committen**: `release/release.json`, `release/release.json.sig`, `release/release.pub`. Die Binaries nie.
9. [ ] **Pushen** (`git push origin main`), dann **taggen und den Tag pushen**:

   ```bash
   git tag v0.5.6 && git push origin v0.5.6
   ```

   Der Tag heisst `v` plus die Version aus `Cargo.toml`, sonst bricht die Action ab.
10. [ ] **Action beobachten**: Der Job `release` baut selbst, führt `scripts/check-release.sh` aus, legt einen Entwurf an, lädt hoch, lädt die Bytes zum Vergleich wieder herunter und veröffentlicht. Danach prüft der Job `mirror` die Dateien erneut gegen das signierte Manifest, pusht Tag und Commit als `main` nach Gitea und legt dort dasselbe Release an.
11. [ ] **Release prüfen**: Auf `https://github.com/chLuethy/atvprep-rs/releases` und auf dem Spiegel `https://git.ch-luethy.ch/chl/atvprep-rs/releases` liegen `atvprep-linux-x86_64`, `atvprep-linux-aarch64`, `release.json`, `release.json.sig`, `release.pub`.
12. [ ] **Einzeiler testen** auf einem Linux-Host, auf dem die Vorversion liegt: `curl -fsSL https://raw.githubusercontent.com/chLuethy/atvprep-rs/main/scripts/install.sh | sudo sh` meldet `0.5.5 -> 0.5.6 aktualisiert`; ein zweiter Lauf meldet „bereits aktuell“.
13. [ ] **Produktiv auf `mm`**: nur im Releasefenster, siehe [DEPLOYMENT.md](../DEPLOYMENT.md), Betreiberabnahme vor dem Öffnen des Gates.

## Wenn etwas schiefgeht

- **Die Action meldet abweichende Hashes**: Es wird nichts veröffentlicht. Das Manifest niemals mit den Hashes der Action neu signieren. Ursache suchen: nicht reproduzierbarer Build oder anderer Quelltext als der signierte (Version, Lockfiles, uncommittete Änderungen beim lokalen Build). Danach ab Schritt 4 wiederholen.
- **Upload bricht ab und ein Entwurf bleibt liegen**: Den fehlgeschlagenen Lauf neu starten. Ein unveröffentlichter Entwurf wird gelöscht und neu angelegt; ein veröffentlichtes Release wird nie ersetzt.
- **Nur der Spiegel schlägt fehl**: Das GitHub-Release bleibt gültig. Ursache beheben, dann den Workflow manuell mit `mirror_version` (z. B. `0.5.6`) starten; er spiegelt nur und ist nach Erfolg wiederholbar. Weicht `main` auf Gitea ab, bricht der Push ab: niemand pusht direkt auf den Spiegel, die Abweichung ist also zu klären, nicht zu überschreiben.
- **Tag falsch gesetzt**: Bevor ein Release veröffentlicht ist, darf der Tag gelöscht und neu gesetzt werden. Ein veröffentlichtes Release wird nicht überschrieben, sondern durch eine höhere Version ersetzt.
- **Rückfall auf eine ältere Version**: `atvprep deploy rollback` mit dem signierten Manifest der älteren Version, siehe [DEPLOYMENT.md](../DEPLOYMENT.md). Das Datenbankschema wird dabei nicht zurückgesetzt.
