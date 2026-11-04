# Binary gehört root, der Manager läuft unprivilegiert

Auf `mm` liegt die Binary unter `/opt/atvprep` im Besitz von root; `atvprep-manager` und `atvprep-hook` sind Symlinks darauf. Der Manager läuft als eigener Dienstbenutzer ohne Schreibrecht darauf und besitzt nur Konfiguration, Token und Datenbank unter `/srv/atvprep-rust`. Das Releasefenster tauscht die Binary als root. So kann ein kompromittierter Dienst seine eigene Programmdatei nicht ersetzen.

Die Hooks in den Containern sind die Ausnahme: Sie liegen im beschreibbaren `/config` und werden von `deploy servarr` aktualisiert; ihre Echtheit sichert die Signaturprüfung gegen den Vertrauensschlüssel.
