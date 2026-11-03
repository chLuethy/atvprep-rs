# atvprep

atvprep bereitet Medien für Apple TV auf und bietet einen Manager mit Weboberfläche. Dieses Repository enthält die Rust-Implementierung, das Webfrontend und den Servarr-Hook. Für Betrieb, Build und Hook wird keine Python-Laufzeit benötigt.

Der Manager verwendet eine eigene SQLite-Datenbank. Eine Datenbank der früheren Implementierung kann nicht übernommen werden. Der produktive Einsatz und die Zielplattformen müssen vor der Umstellung separat abgenommen werden.

## Voraussetzungen

Rust gemäß `rust/rust-toolchain.toml`, Node.js 22 mit npm sowie für die Medienverarbeitung `ffmpeg`, `ffprobe`, `mp4info` und `mp4dump` (Bento4). Für die vollständige Rust-Prüfung ist außerdem `cargo-deny` erforderlich.

```sh
./rust/check.sh
cd frontend
npm ci
npm run typecheck
npm test
npm run build
```

Die Tests verwenden eingecheckte Referenzdaten und benötigen kein zusätzliches Oracle. Die Binary wird mit `cargo build --locked --release --manifest-path rust/Cargo.toml -p atvprep` gebaut. `atvprep`, `atvprep-manager` und `atvprep-hook` sind Einstiegspunkte derselben ausführbaren Datei; für die Aliasnamen genügt ein Symlink.

Installation, Konfiguration, Servarr und Releasefenster stehen in [DEPLOYMENT.md](DEPLOYMENT.md). Der Container- und Multiarch-Build ist in [rust/release/README.md](rust/release/README.md) beschrieben.
