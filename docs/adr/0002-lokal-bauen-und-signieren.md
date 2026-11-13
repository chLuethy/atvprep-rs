# Releases werden lokal gebaut und signiert; CI prüft und veröffentlicht nur

Binaries werden auf dem Rechner des Betreibers gebaut und mit einem privaten Schlüssel signiert, der nie in Gitea oder CI liegt. Die Gitea-Action prüft nur Hashes, Signatur, Version und ELF-Architektur der eingecheckten Dateien und veröffentlicht genau diese Bytes; sie kompiliert nie. Der Bau in der Action war extrem langsam, und ein Schlüssel in CI wäre ein größeres Risiko als ein lokaler Build.

Produktiv zählt nur `linux-x86_64`; `linux-aarch64` wird per ELF-Header und Hash geprüft, nicht per Emulation gestartet.

## Nachtrag: Action auf GitHub

Seit ADR 0009 läuft die Action auf GitHub statt auf Gitea; der Schlüssel liegt weder in GitHub noch in Gitea. Das Prüfprinzip gilt unverändert, das Einchecken der Binaries hat ADR 0005 abgelöst.
