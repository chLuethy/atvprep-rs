# CI baut die Binaries reproduzierbar nach, signiert wird lokal

ADR 0002 hielt die Binaries im Repository, weil der Bau in der Action langsam war und ein Schlüssel in CI ein größeres Risiko wäre als ein lokaler Build. Jedes Release brachte so etwa 36 MB nicht deltierbare Historie.

Der Schlüssel bleibt lokal; das Signieren ändert sich nicht. Neu ist, dass die Binaries nicht mehr eingecheckt werden. Der Betreiber baut weiterhin lokal und signiert das Manifest mit den Hashes dieses Builds. Die Action baut dieselben Binaries aus dem getaggten Quelltext in der gepinnten Container-Umgebung (Image-Digests, Lockfiles, fester Quellpfad, `SOURCE_DATE_EPOCH`) und veröffentlicht nur, wenn ihr eigener Build zu den signierten Hashes passt. Ein abweichender Build bricht den Lauf ab; sein Ergebnis wird nie signiert.

Das ist stärker als zuvor: Die veröffentlichten Bytes sind nachweislich aus dem getaggten Quelltext entstanden, nicht nur vom Betreiberrechner behauptet. Es setzt voraus, dass der Build reproduzierbar bleibt. Ursprünglich lief der aarch64-Build in CI unter QEMU und war langsam; produktiv zählt weiter nur `linux-x86_64`.

Die Release-Dateien im Repository sind nur noch Manifest, Signatur und öffentlicher Schlüssel. Die zuvor eingecheckten Binaries von v0.5.5 bleiben in der Historie.

## Nachtrag: Nachbau auf GitHub

Seit ADR 0009 baut eine GitHub-Action auf GitHub-gehosteten Runnern nach und veröffentlicht auf GitHub; Gitea erhält die bereits verifizierten Bytes als Spiegel. Reproduzierbarkeit und Vergleich mit den signierten Hashes gelten unverändert.

## Nachtrag: Beide Plattformen nativ und parallel nachbauen

GitHub baut `linux-x86_64` und `linux-aarch64` parallel auf nativen Runnern und prüft die Installation auf beiden Architekturen. Beim v0.8.0-Release nahm allein die ARM64-Kompilierung unter QEMU rund 55 Minuten in Anspruch; der native Probelauf mit demselben signierten Manifest bestand in 4:11 Minuten. Beide Assets bleiben Teil des Releases. Ein separater Job prüft die zum Upload vorgesehenen Bytes erneut gegen das signierte Manifest. Der unabhängige Nachbau und die lokale Signatur bleiben damit erhalten.
