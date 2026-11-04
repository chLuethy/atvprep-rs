# Ein root-eigener Vertrauensschlüssel auf `mm`, read-only in die Container

Der Schlüssel im Repo (`release/release.pub`) liegt neben dem Manifest und beweist daher nichts. Auf `mm` gibt es eine einzige, auf anderem Weg dorthin gebrachte Datei (root-eigen, nicht gruppenschreibbar), die allen `--trusted-key`-Aufrufen dient und read-only in Sonarr und Radarr eingebunden wird. Eine Kopie im beschreibbaren `/config`-Volume wäre dort so angreifbar wie die Binary selbst. Der Fingerabdruck wird zusätzlich außerhalb von Gitea festgehalten.

Ein Schlüsselwechsel und ein in die Binary einkompilierter Schlüssel sind bewusst nicht gelöst und brauchen einen eigenen ADR.
