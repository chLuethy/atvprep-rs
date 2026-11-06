# Rust-Port: Betrieb und Cutover

Der Manager ist noch nicht produktiv freigegeben. Er verwendet eine neue, separate Datenbank; eine Datenbank der früheren Implementierung wird beim Start zurückgewiesen.

## Installation und Update (ein Befehl)

```bash
curl -fsSL https://git.ch-luethy.ch/chl/atvprep-rs/raw/branch/main/scripts/install.sh | sudo sh
```

`scripts/install.sh` installiert die Binary nach `/usr/local/bin/atvprep` samt Verknüpfungen `atvprep-manager` und `atvprep-hook`, oder aktualisiert sie, wenn sie schon vorhanden ist (gleiche Version: „bereits aktuell“, Exitcode 0; ein Downgrade verweigert es). Es unterstützt Linux x86_64 und aarch64 und braucht `curl`, `ssh-keygen` und `sha256sum`.

Vertrauen: Das Skript selbst ist nicht vertrauenswürdig. Es führt nur eine Binary aus, deren signiertes Manifest gegen den Vertrauensschlüssel `/etc/atvprep/release.pub` verifiziert (Namespace `atvprep-release`) und deren SHA-256 zum Manifest passt. Fehlt die Datei, lädt das Skript den Schlüssel einmalig aus dem Release (Trust on first use, ADR 0003), zeigt den Fingerabdruck an und legt ihn root-eigen ab. Mit `ATVPREP_KEY_FINGERPRINT=SHA256:…` bricht es bei einem anderen Schlüssel ab. Spätere Läufe laden den Schlüssel nie wieder.

Servarr-Hook (einmalig je Anwendung; der Pfad endet auf `atvprep-hook` im `/config` des Containers):

```bash
curl -fsSL https://git.ch-luethy.ch/chl/atvprep-rs/raw/branch/main/scripts/install.sh | sudo sh -s -- servarr sonarr /pfad/zu/sonarr-config/atvprep-hook
```

Der Pfad wird in `/etc/atvprep/hooks` eingetragen; jeder weitere Lauf aktualisiert zuerst die Binary und danach alle eingetragenen Hooks. Ein fehlgeschlagener Hook beendet den Lauf mit Exitcode 1, die Binary bleibt neu.

Umgebungsvariablen: `ATVPREP_VERSION` (feste Version), `ATVPREP_DEST`, `ATVPREP_TRUSTED_KEY`, `ATVPREP_KEY_FINGERPRINT`, `ATVPREP_HOOKS`. Auf `mm` läuft ein Update der Binary produktiv nur im Releasefenster (unten); der Befehl ändert weder Gate noch Datenbank.

## Manuell und offline installieren

Die Binary benötigt für Medienarbeiten `ffmpeg`, `ffprobe`, `mp4info` und `mp4dump`.
macOS und Linux verwenden dieselben Multicall-Einstiegspunkte: `atvprep`, `atvprep-hook` und
`atvprep-manager`. Ein Symlink auf dieselbe Binary reicht für den jeweiligen Namen.
Release-Plattformen sind `linux-x86_64`, `linux-aarch64`, `macos-x86_64` und `macos-aarch64`;
das ist die Manifest-Allowlist, keine Aussage über abgeschlossene Tests aller Plattformen.
Veröffentlicht werden derzeit nur die Linux-Assets, der Einzeiler unterstützt macOS nicht.

Der unabhängig bereitgestellte öffentliche OpenSSH-Ed25519-Schlüssel muss dem lokalen Benutzer
oder root gehören. Schlüsseldatei und Installationsverzeichnis dürfen nicht durch Gruppe oder
andere Benutzer schreibbar sein. Der Signaturnamespace ist `atvprep-release`.

```bash
atvprep version
atvprep manager version
atvprep deploy install --manifest release.json --signature release.json.sig --trusted-key release.pub --asset-file release-binary --destination ./bin/atvprep
```

Manifest und Signatur können auch HTTPS-URLs sein. Ohne `--asset-file` lädt der Installer die
Manifest-URL des zur Maschine passenden Assets. HTTPS-Redirects sind deaktiviert. Signatur,
Plattform, Größe und SHA-256 werden vor Installation oder Start geprüft. Der Austausch ist atomar;
eine falsche Versionsausgabe stellt die vorherige Binary automatisch wieder her.
`update` verweigert gleiche/ältere Versionen; `rollback` erlaubt bewusst eine signierte ältere Version.

## Konfiguration und Start

Konfiguration und Credentials gehören in ein privates Verzeichnis, Dateimodus `0600`.
Den Master-Token in einer separaten geschützten Datei bereitstellen; niemals in Argumente oder
Browser-Storage schreiben. `approval_secret` ist ein separates, dauerhaftes Secret für manuelle
Prepare-Freigaben. Ein Wechsel entwertet noch wartende Freigaben.

```toml
[server]
bind_host = "127.0.0.1"
bind_port = 8765
allowed_origins = ["http://127.0.0.1:8765"]
[storage]
database_path = "/srv/atvprep-rust/manager.sqlite"
[library]
roots = ["/srv/media"]
[auth]
master_token_file = "/srv/atvprep-rust/master-token"
approval_secret = "REPLACE_WITH_A_PRIVATE_RANDOM_SECRET"
session_cookie_secure = false
[atvprep]
ffprobe = "/usr/bin/ffprobe"
```

Unsichere Cookies sind nur für Loopback-Erprobungen vorgesehen. Hinter HTTPS `session_cookie_secure`
aktivieren und die tatsächlichen Origins eintragen. Pfade an die Maschine anpassen.

```bash
atvprep manager api --config config.toml
```

Die Weboberfläche ist in die Binary eingebettet (`frontend/dist` wird beim Bauen eingelesen; der Release-Build baut sie zuerst). `--web-dist VERZEICHNIS` oder ein `web_dist` neben der Binary hat Vorrang und ersetzt sie, etwa für eine abweichende Oberfläche. Eine Binary, die ohne gebautes Frontend kompiliert wurde, braucht eines dieser beiden.

Dieser Prozess startet API, Scanner, Executor und Wartung gemeinsam. Ein separater Worker wird nicht benötigt. SIGINT/SIGTERM beendet Dienst und Worker kontrolliert. `/api/health/live`
meldet den laufenden HTTP-Dienst; `/api/health/ready` prüft SPA, Worker, Datenbank und offenes Gate.
Während Wartung liefert Readiness absichtlich 503. `/api/v1/version` nennt die ausgelieferte Version.

## Servarr-Hook

Die Rust-Integration verwendet einen direkt installierten signierten Hook statt eines Python-venv-Runtime-Markers. Auf dem Zielsystem beziehungsweise im Zielcontainer ausführen:

```bash
atvprep deploy servarr --application sonarr --manifest release.json --signature release.json.sig --trusted-key release.pub --asset-file release-binary --destination /config/atvprep-hook
```

`--application` akzeptiert `sonarr`, `radarr` oder `whisparr`. Der Zielname muss `atvprep-hook` sein; im Servarr-Custom-Script-Feld diesen stabilen Pfad eintragen. Der Prozess muss zur Asset-Plattform passen und Schreibrecht auf das geschützte Zielverzeichnis besitzen. Identische verifizierte Bytes werden nicht erneut ausgetauscht; gleiche Version mit anderen Bytes und Downgrades werden abgelehnt. Updates laufen atomar und unter einer OS-Sperre. Nach einem Wechsel benötigt der nächste Hook-Aufruf keinen Container-Neustart.

Bestehende Installationen bleiben während der Erprobung erhalten. Bei einem separaten Rust-Container kann alternativ der mitgelieferte Hook `/usr/local/bin/atvprep-hook` mit einem Image-Update gewechselt werden. Ein Hook-Aufruf ohne Medienargumente endet erwartungsgemäß mit Exitcode 2; `--version` liefert die Release-Version.

## Backup und Releasefenster

Vor Ausführung sämtliche API-/Worker-Dienste und Servarr-Imports anhalten. Das lokale CLI muss
unter dem Eigentümer der Rust-Datenbank laufen. Ein frisches privates Backup-Verzeichnis anlegen.
Der folgende Befehl validiert standardmäßig nur; er schreibt weder Backup noch Journal oder Gate.

```bash
atvprep deploy release-window --database manager.sqlite --backup-dir backup --manifest release.json --signature release.json.sig --trusted-key release.pub --asset-file release-binary --destination ./bin/atvprep
```

Zur tatsächlichen Ausführung dieselben Argumente mit `--execute --services-stopped --imports-stopped`
verwenden. `--drain-timeout` begrenzt das Warten auf offene Arbeit (Standard 60 Sekunden).
Die neue Binary wird verifiziert und gestaged, dann das Gate geschlossen, offene Arbeit drainiert,
ein SQLite-Online-Backup mit Integritäts- und Tabellenzählungsprüfung erstellt und die Offline-Migration
unter Generation-CAS begonnen. Die signierte neue Binary führt `manager migrate` aus. Nach Aktivierung
wird die Migration beendet; das Gate bleibt **geschlossen**. Das JSON-Ergebnis enthält Generation,
Ausführungsstatus und Backup-Status. Exitcode 2 bedeutet Drain-Timeout, andere Fehler 1.

`backup/manager-backup.sqlite` ist die abgeschlossene Sicherung; `backup/window.jsonl` ist ein nach
jedem Schritt synchronisiertes Journal. Bei Fehlern bleiben Gate und Journal erhalten. Kein blinder
Retry: Journal und Gate prüfen, Fehler beheben und für einen neuen Versuch ein frisches
Backup-Verzeichnis verwenden. Es gibt keinen automatischen Schema-Downgrade und keine automatische
Wiederaufnahme eines fehlgeschlagenen Fensters.

```bash
atvprep deploy gate status --database manager.sqlite
atvprep deploy gate permits --database manager.sqlite
atvprep deploy gate history --database manager.sqlite
atvprep deploy gate reap-permits --database manager.sqlite
```

Reaping entfernt nur nachweislich beendete lokale Prozessidentitäten. Unbekannte Identitäten bleiben
stehen; ein bewusstes `abandon-permit` verlangt Permit-ID und Begründung bei geschlossenem Gate.
Nach geprüftem Release und Betreiberabnahme das Gate mit der **aktuellen** Generation öffnen:

```bash
atvprep deploy gate open --database manager.sqlite --generation 4
```

Die Zahl ist ein Beispiel, kein fester Wert. Bei fehlgeschlagener Offline-Migration zunächst Ursache
und Schema prüfen; `finish-migration` ist keine pauschale Fehlerquittierung.

## Rollback und Nachweise

```bash
atvprep deploy rollback --manifest previous.json --signature previous.json.sig --trusted-key release.pub --asset-file previous-binary --destination ./bin/atvprep
```

Dienste und Imports vorher stoppen, Gate geschlossen halten. Ein Binary-Rollback setzt das Schema
nicht zurück. Eine Datenbank-Rücksicherung nur offline nach Prüfung der Schema-Kompatibilität
vornehmen; SQLite-WAL/SHM-Dateien dürfen nicht zu einer anderen Sicherung gehören.

**Zeitpläne und Rückfall:** Das Schema ist seit dem Release mit den Zeitplänen (Schema-Version 1 auf 5,
Migrationen 0002 bis 0005) **nicht abwärtskompatibel**. Eine ältere Binary verweigert die Datenbank,
weil deren `user_version` neuer ist als sie kennt; die Altspalten `days_mask` und `time_of_day` ändern
daran nichts, und 0003 baut `schedules` mit einer erweiterten CHECK-Bedingung neu auf. Ein Rückfall
auf eine Binary vor diesem Release verlangt deshalb die Wiederherstellung der Datenbank-Sicherung vor
dem Release (siehe Glossar „Rückfall“); Daten seit dem Releasefenster gehen dabei verloren.

```bash
CARGO_TARGET_DIR=/private/tmp/atvprep-target ./rust/check.sh
```

Die Rust-Tests prüfen unter anderem CLI, Hook und Deployment. Container-Smoke-Tests sind in der Release-Anleitung beschrieben.

Produktiver Cutover benötigen eine gesondert
dokumentierte Betreiberentscheidung. Die technische Suite erteilt diese Freigabe nicht.
