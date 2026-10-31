# k-lani-link-Relay auf einer WordPress-Seite einrichten

Mit diesem Plugin kann eine bestehende WordPress-Seite als verschlüsselte Zwischenstation
für k-lani-link dienen. Die Seite speichert dabei nur verschlüsselte Datenpakete, die
nach 30 Sekunden ablaufen und bei der nächsten Anfrage oder dem nächsten Cron-Lauf
gelöscht werden; sie kann keine Inhalte lesen. Gedacht für **ein bis zwei gleichzeitige
Nutzer** auf einem kleinen Hosting-Paket.

## Voraussetzungen

- WordPress 6 oder neuer mit **PHP 8.2 oder neuer**
- HTTPS für die Seite (Let's-Encrypt-Zertifikat reicht)
- **„Schöne“ Permalinks**: *Einstellungen → Permalinks* auf etwas anderes als „Einfach“
  stellen (z. B. „Beitragsname“). Sonst ist `/wp-json/` nicht erreichbar.
- Die Erlaubnis der Person, der die Seite gehört

## Installation

1. Plugin-Ordner `k-lani-link-relay` mit `k-lani-link-relay.php`, `relay.php`,
   `k-lani-relay.php` und `uninstall.php` als ZIP über *Plugins → Installieren → Plugin hochladen* einspielen
   oder per SFTP nach `wp-content/plugins/` kopieren.
2. Plugin aktivieren.
3. *Einstellungen → k-lani-link Relay* öffnen. Dort stehen zwei **Relay-URLs**: die
   schnelle `https://example.org/wp-content/plugins/k-lani-link-relay/relay.php` (startet
   WordPress nicht bei jeder Anfrage) und als Ausweichweg
   `https://example.org/wp-json/k-lani-link`, falls der Hoster oder ein
   Sicherheits-Plugin direkte PHP-Aufrufe in Plugin-Ordnern sperrt.
4. **Zugang anlegen**: unter „New access“ einen Namen eintragen (z. B. „Laptop“) und
   **„Create token pair (host + client)“** klicken. Das erzeugt zwei Tokens im selben
   Bereich, eines für den Host, eines für den Client. Beide werden **nur einmal**
   angezeigt: in die Dateien `host-token` und `client-token` kopieren und privat halten
   (`chmod 600`). Die Seite speichert nur Hashes. Ein verlorenes Token kann nicht
   wiederhergestellt, nur widerrufen und neu erstellt werden. Geht der Laptop verloren,
   nur das Client-Token widerrufen; der Host bleibt erreichbar.
   Optional im selben Formular oder später in der Tabelle (Speichern mit „Save rules“):
   - **Valid until**: letzter gültiger Tag; danach wird das Token abgelehnt.
   - **Allowed addresses**: nur diese Adressen oder Netze, z. B. `203.0.113.0/24`.
   - **Addresses per day**: höchstens so viele verschiedene Adressen in 24 Stunden
     (z. B. 8 für den Host, 12 für einen mobilen Client; IPv4 und IPv6 zählen getrennt).
     Bekannte Adressen bleiben erlaubt; gespeichert werden nur gesalzene Hashes.
     Wird ein Endpunkt abgelehnt, versucht er es nicht weiter: Der Host beendet sich
     und muss neu gestartet werden. Deshalb großzügig wählen.
   Adressregeln sehen die Adresse, die sich mit der Webseite verbindet. Steht ein CDN
   oder Proxy davor (z. B. Cloudflare), ist das dessen Adresse: dann leer lassen.
   Wird der Bereich eines Tokens geändert, brechen dessen laufende Verbindungen ab und
   bauen sich neu auf.
5. **Wartezeit und Grenzen**: Wartezeit 5 Sekunden ist ein guter Start. „Gleichzeitig
   wartende Anfragen“ auf etwa die Hälfte der PHP-Prozesse des Pakets setzen (Standard 3;
   bei 8 Prozessen 4). „Parallele Schreibvorgänge“ bei 2 lassen. Die Endpunkte lesen diese
   Werte von der Seite, halten sie ein und drosseln sich bei Überlast selbst weiter.
6. **Schutz vor Rateversuchen** (optional): „Refused requests per minute and address“
   z. B. auf 20 setzen. Eine Adresse mit mehr Fehlversuchen bekommt dann für die
   eingestellten Minuten `429`. Gültige Tokens werden nie gesperrt.
7. **Aufräumen ohne Besucher**: Das Plugin löscht abgelaufene Pakete bei jeder Anfrage
   und einmal pro Minute über WP-Cron. WP-Cron läuft aber nur, wenn jemand die Seite
   aufruft. Bietet der Hoster echte Cronjobs an, einen Cronjob einrichten, der jede Minute
   `https://example.org/wp-cron.php` aufruft. Ohne Besucher und ohne Cronjob bleiben
   abgelaufene (verschlüsselte) Pakete bis zur nächsten Anfrage liegen.

## Koppeln per Code

Statt Ordner zu kopieren, zeigt der Rechner mit dem Modell einen elfstelligen Code, den
man am anderen Rechner eintippt (zehn Minuten gültig, ein Versuch):

```sh
k-lani-link offer-https --dir neues-pairing \
  --relay-url https://example.org/wp-content/plugins/k-lani-link-relay/relay.php \
  --admission-token-file host-token \
  --client-admission-token-file client-token
```

Am anderen Rechner `k-lani-link join-https --dir client --relay-url <dieselbe URL>`
aufrufen und den Code eingeben. Das Relay sieht dabei nur öffentliche Schlüsseltausch-
Nachrichten und verschlüsselte Daten. Standardmäßig sind vier Codes pro Bereich und 64
auf dem ganzen Relay gleichzeitig offen. Lehnt eine Web-Firewall des Hosters Binärdaten
ab, scheitert das Koppeln per Code; dann den Weg unten mit `init-https` nehmen.

## Pairing auf dem eigenen Rechner erzeugen

```sh
k-lani-link init-https --dir neues-pairing \
  --relay-url https://example.org/wp-content/plugins/k-lani-link-relay/relay.php \
  --admission-token-file host-token \
  --client-admission-token-file client-token
```

Mit nur einem Token („Create single token“) entfällt `--client-admission-token-file`;
beide Seiten nutzen dann dasselbe Token.

Im erzeugten `client/client.toml` und `host/host.toml` im Abschnitt `[https]`
`poll_wait_seconds = 5` setzen (nicht höher als die Wartezeit im Plugin). Den
`client`-Ordner zum Kunden mitnehmen, den `host`-Ordner auf dem Rechner mit dem Modell
lassen. Sperrt eine Firewall PUT oder Binärdaten, stellen sich beide Seiten selbst auf
POST bzw. JSON um; `post_writes = true` beschleunigt nur den ersten Verbindungsaufbau.

## Was die Seite zu sehen bekommt

Nur verschlüsselte Pakete, IP-Adressen und Zeitpunkte der Anfragen, wie bei jedem
Webseitenbesuch. Pakete laufen nach 30 Sekunden ab, Sitzungen nach zwei Minuten;
gelöscht werden sie bei der nächsten Anfrage oder dem nächsten Cron-Lauf (siehe Schritt 7
der Installation). Für die Regel „Addresses per day“ und die Fehlversuchs-Bremse liegen
dort zusätzlich gesalzene Hashes von Adressen, höchstens einen Tag lang. Der Speicherordner liegt unter `wp-content/uploads/` mit zufälligem Namen und
ist per `.htaccess` gesperrt. Tägliche Backups des Hosters können kurzlebige
verschlüsselte Pakete enthalten; lesbar sind sie nicht.

## Wenn es nicht funktioniert

| Anzeichen | Ursache und Abhilfe |
| --- | --- |
| `404` auf `/wp-json/k-lani-link/...` | Permalinks auf „Einfach“, oder ein Sicherheits-Plugin sperrt die REST-API. Permalinks umstellen bzw. den Namensraum `k-lani-link` freigeben. |
| `401` | Token falsch, widerrufen, abgelaufen, Adresse nicht erlaubt oder zu viele Adressen heute; oder der Hoster entfernt den `Authorization`-Header. Regeln in der Tabelle prüfen; bei eigenem `.htaccess` die Weitergabe von `Authorization` prüfen. |
| `403` dauerhaft | Firewall-Regel des Hosters, die auch POST/JSON sperrt. Hoster bitten, die Relay-URL freizugeben. |
| Langsam, viele Sekunden Verzögerung | Zu wenige PHP-Prozesse für die Zahl der Nutzer. Weniger gleichzeitige Nutzer oder größeres Paket; Wartezeit nicht erhöhen. |
| Plötzlich gesperrt (`429`, Captcha-Seite) | DDoS-/Ratenschutz des Hosters. Wartezeit beibehalten, nicht senken; ggf. Hoster fragen. |

## Entfernen

Deaktivieren schließt den schnellen Zugang sofort. Löschen entfernt zusätzlich
Einstellungen, Token-Hashes und den Speicherordner.
