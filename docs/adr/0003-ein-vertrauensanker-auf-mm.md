# Ein root-eigener Vertrauensschlüssel auf `mm`, read-only in die Container

Der Schlüssel im Repo (`release/release.pub`) liegt neben dem Manifest und beweist daher nichts. Auf `mm` gibt es eine einzige, auf anderem Weg dorthin gebrachte Datei (root-eigen, nicht gruppenschreibbar), die allen `--trusted-key`-Aufrufen dient und read-only in Sonarr und Radarr eingebunden wird. Eine Kopie im beschreibbaren `/config`-Volume wäre dort so angreifbar wie die Binary selbst. Der Fingerabdruck wird zusätzlich außerhalb von Gitea festgehalten.

Ein Schlüsselwechsel und ein in die Binary einkompilierter Schlüssel sind bewusst nicht gelöst und brauchen einen eigenen ADR.

## Nachtrag: Erstinstallation per `scripts/install.sh`

Auf Hosts ohne vorhandene Schlüsseldatei lädt das Skript den Schlüssel einmalig aus dem Release, zeigt den Fingerabdruck an und legt ihn root-eigen ab (Trust on first use). Dieser eine Schritt vertraut Gitea samt TLS; `ATVPREP_KEY_FINGERPRINT` pinnt ihn strikt. Danach gilt wieder nur die lokale Datei, und das Skript lädt den Schlüssel nie erneut. Auf `mm` bleibt die Datei der unabhängig platzierte Anker.
