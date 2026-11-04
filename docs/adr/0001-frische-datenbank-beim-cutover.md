# Cutover mit frischer Rust-Datenbank, Altsystem bleibt erhalten

Rust öffnet keine Datenbank des Altsystems und migriert sie nicht; auf `mm` startet es mit einer neuen. Die Python-Historie (Jobs, Scan-Ergebnisse, Zeitpläne) geht bewusst nicht über. Das Altsystem wird beim Cutover gestoppt und deaktiviert, aber nicht gelöscht; seine Löschung ist ein eigener späterer Beschluss. Rückfall zum Altsystem ist möglich, solange es unberührt ist; Rust-Daten seit dem Cutover und Änderungen an Mediendateien werden dadurch nicht rückgängig gemacht.

Altsystem und Rust verarbeiten nie gleichzeitig dieselben Medien.
