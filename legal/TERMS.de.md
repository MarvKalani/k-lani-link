# Kauf- und Nutzungsbedingungen für k-lani-link 1.x

**Entwurf vom 28. Oktober 2026 zur rechtlichen Prüfung. Noch nicht als neue
Checkout-Bedingungen aktiviert.** [English](TERMS.en.md)

## 1. Anbieter und Geltungsbereich

Anbieter ist Marvin Kalani, Zum Bruch 3a, 28816 Stuhr, Deutschland,
E-Mail: support@kalanis.de. Diese Bedingungen sind für künftige Käufe einer
kommerziellen k-lani-link-Lizenz durch Verbraucher und Unternehmer vorgesehen.
Sie werden vor der Bestellung angezeigt und nur bei wirksamer Einbeziehung
Vertragsbestandteil. Die bloße Installation oder Verwendung der Software ersetzt
keine Zustimmung im Kaufvorgang. Frühere Käufe werden nicht nachträglich geändert.

Die [kommerzielle Lizenz](../COMMERCIAL-LICENSE.md) regelt insbesondere Nutzerzahl,
Nutzungsrechte und Versionsumfang. Die [BSL 1.1](../LICENSE) und Lizenzen anderer
Komponenten bleiben bestehen. Diese Kaufbedingungen schränken eine unabhängig
bereits bestehende Berechtigung zur kostenlosen Nutzung nicht nachträglich ein.

## 2. Leistung und Systemgrenzen

k-lani-link ist selbst betriebene Software zur authentifizierten, verschlüsselten
Übertragung unterstützter KI-API-Anfragen und Antworten zwischen zwei vom Nutzer
gewählten Rechnern. Dazu gehören Streaming und die dokumentierte Begrenzung
paralleler Anfragen. Die optional aktivierte Modellverwaltung startet und beendet
vorab freigegebene lokale Modellprozesse. Sie gestattet keine beliebigen
Fernbefehle oder vom Client vorgegebenen Dateipfade.

k-lani-link erzeugt selbst keine KI-Antworten und führt keine von einem Modell
vorgeschlagenen Dateiänderungen, Shellbefehle oder Werkzeugaufrufe aus. Welche
Aktionen daraus entstehen, bestimmen das angebundene Modell, der Coding-Agent,
das Harness, Werkzeuge und deren vom Betreiber eingeräumte Rechte. Der Anbieter
schuldet keine bestimmte inhaltliche Richtigkeit, Sicherheit oder wirtschaftliche
Verwendbarkeit solcher fremden KI-Ergebnisse. Die Verantwortung für eigene
Pflichtverletzungen oder Fehler von k-lani-link bleibt hiervon unberührt.

Es werden keine Modellgewichte, Rechenleistung, gehostete Inferenz oder TURN-Dienste
verkauft. TURN erfordert eine ausdrückliche Konfiguration eines selbst beschafften
Dienstes. Internet allein garantiert keinen Verbindungsaufbau. NAT, Firewalls, Proxys,
Endgeräteschutz und Netzrichtlinien können direkte Verbindungen verhindern.
Eine Funktion in jedem Netzwerk wird nicht zugesagt. Die Verantwortung für
eigene Softwaremängel bleibt davon unberührt. Maßgeblich
sind der konkret beschriebene Funktionsumfang, die Systemanforderungen und die vor
dem Kauf mitgeteilten Einschränkungen. Aktuell ist das Referenzsetup unter Linux
x86_64 mit Docker getestet; native Windows- und macOS-Pakete sind nicht veröffentlicht.
Der Basalt-Adapter benötigt eine unterstützte NVIDIA-GPU und passende lokale Gewichte.

## 3. Preis, Vertrag und Bereitstellung

Der Preis beträgt einmalig 4,99 EUR pro menschlichem Nutzer für alle Versionen 1.x,
einschließlich der im EUR-Preis enthaltenen anwendbaren Steuern. Maßgeblich ist der
vor Bestellung angezeigte Gesamtbetrag für die gewählte Nutzerzahl. Es gibt kein
Abonnement und keine Online-Aktivierung. Eine etwaige Währungsumrechnung wird vom
Zahlungsanbieter separat angezeigt.

Die Bestellung erfolgt über den bezeichneten Stripe-Checkout. Der Vertrag kommt
mit erfolgreicher Zahlung und Kaufbestätigung zustande. Verkäufer und Lizenzgeber
ist Marvin Kalani; Stripe ist Zahlungsdienstleister und wird dadurch nicht zum
Anbieter der Software. Lizenz und Zugang zum vereinbarten Softwarestand werden
anschließend digital bereitgestellt. Es erfolgt kein Warenversand. Kaufbestätigung,
Lizenzbedingungen und die einbezogene Fassung dieser Bedingungen sind dem Käufer
in dauerhaft speicherbarer Form zur Verfügung zu stellen.

## 4. Betrieb, fremde Systeme und Daten

Der Betreiber wählt Gegenstellen, Modelle, Agenten, deren Werkzeuge und
Berechtigungen. In durch Dritte verwalteten Netzen, insbesondere beim Arbeitgeber,
muss er die zuständige IT-Administration vor dem Einsatz informieren und die
erforderlichen Freigaben einholen. Netzwerk-, Sicherheits- und Datenschutzvorgaben
sind einzuhalten. Änderungen an Schutz- oder Netzkonfigurationen dürfen nur im
Rahmen der jeweiligen Berechtigung erfolgen. Er soll KI-Ergebnisse vor Ausführung prüfen, mit geringen Rechten
arbeiten und angemessene Sicherungen wichtiger Daten vorhalten. Er ist für die
Befugnis verantwortlich, verwendete Daten an die gewählte Gegenstelle oder einen
gegebenenfalls angebundenen Cloud-Anbieter zu übermitteln. Eine ausdrückliche
Cloud-Anbindung kann Daten an diesen Anbieter übertragen.

Diese Betriebshinweise sind keine pauschale Bestätigung besonderer Fachkenntnis,
kein Haftungsverzicht und keine Umkehr der gesetzlichen Beweislast. Die gesetzliche
Bewertung eines etwaigen Mitverschuldens bleibt bestehen. Die Software erteilt
keine automatische DSGVO-Zertifizierung und ersetzt keine organisatorischen
Datenschutzmaßnahmen. Zahlungs- und Kontaktdaten werden getrennt vom Inferenzkanal
verarbeitet; der Lizenzkauf aktiviert keine Telemetrie in den Inferenzcontainern.

## 5. Mängelrechte, Aktualisierungen und Unterstützung

Gesetzliche Mängelrechte bleiben bestehen. Die Bezeichnung als MVP oder die
Offenlegung des Quellcodes beseitigt sie nicht. Auch gesetzlich erforderliche
Fehlerbehebungen und Sicherheitsaktualisierungen bleiben geschuldet. Ein Anspruch
auf bestimmte neue Funktionen, Version 2 oder spätere Hauptversionen, garantierte
Antwortzeiten oder ein Service-Level-Agreement wird nicht zugesagt.

## 6. Haftung

Der Anbieter haftet unbeschränkt bei Vorsatz und grober Fahrlässigkeit, bei einer
schuldhaften Verletzung von Leben, Körper oder Gesundheit, bei arglistigem
Verschweigen eines Mangels, im Umfang einer ausdrücklich übernommenen Garantie
sowie in Fällen zwingender gesetzlicher Haftung, insbesondere nach dem
Produkthaftungsgesetz.

Bei einfacher Fahrlässigkeit haftet der Anbieter für die Verletzung wesentlicher
Vertragspflichten, deren Erfüllung die ordnungsgemäße Durchführung des Vertrags
überhaupt erst ermöglicht und auf die der Kunde regelmäßig vertrauen darf. In
diesen Fällen ist die Haftung auf den bei Vertragsschluss vorhersehbaren,
vertragstypischen Schaden begrenzt. Im Übrigen ist die Haftung für einfache
Fahrlässigkeit ausgeschlossen. Die unbeschränkte Haftung des vorigen Absatzes
bleibt in jedem Fall bestehen. Diese Regeln gelten entsprechend für gesetzliche
Vertreter und Erfüllungsgehilfen.

Für Schäden, die ausschließlich auf fremden KI-Ergebnissen oder selbständig
konfigurierten Handlungen fremder Agenten und Werkzeuge beruhen und nicht durch
eine Pflichtverletzung des Anbieters verursacht wurden, wird keine eigenständige
Erfolgshaftung übernommen. Eigene Softwarefehler werden nicht allein wegen des
Einsatzes eines fremden Modells oder Agenten von der Haftung ausgenommen.

Eine Rückzahlung des Lizenzpreises ist von einem Schadensersatzanspruch zu
unterscheiden. **Eine allgemeine Begrenzung sämtlicher Ansprüche auf 4,99 EUR oder
den gezahlten Kaufpreis wird nicht vereinbart.**

## 7. Widerruf, Erstattung und anwendbares Recht

Für Verbraucher gelten die [gesonderten Widerrufsinformationen](WITHDRAWAL.de.md).
Diese Fassung verlangt keinen Verzicht auf das Widerrufsrecht für eine sofortige
Bereitstellung. Gesetzliche Erstattungs- und Mängelrechte bleiben erhalten. Eine
berechtigte Rückabwicklung des Kaufs und darüber hinausgehende gesetzliche
Ansprüche richten sich nach ihren jeweiligen Voraussetzungen.

Es gilt deutsches Recht unter Ausschluss des UN-Kaufrechts. Verbraucher behalten
den Schutz zwingender Vorschriften ihres gewöhnlichen Aufenthaltsstaates, soweit
diese anwendbar sind. Gesetzliche Gerichtsstände bleiben unberührt. Individuelle
Vereinbarungen und zwingendes Recht haben Vorrang vor diesen Bedingungen.
