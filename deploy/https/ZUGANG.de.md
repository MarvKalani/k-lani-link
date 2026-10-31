# Zugang zum Relay einschränken

Ohne gültiges Zugangstoken (256 Zufallsbits) beantwortet das Relay jede Anfrage mit
`401`. Mehr braucht es nicht, damit Fremde es nicht nutzen können. Einzige Ausnahme ist
die Client-Seite eines offenen Kopplungsfachs beim Koppeln per Code (siehe unten): Sie
gilt zehn Minuten, für genau einen Versuch, und öffnen kann sie nur ein Token-Inhaber. Die folgenden
Optionen sind zusätzliche Sicherungen für den Fall, dass ein Token in falsche Hände
gerät. Alle sind abgeschaltet, solange sie nicht in `relay.toml` stehen. Geprüft wird im
Arbeitsspeicher, bevor das Relay Daten annimmt.

## Empfohlen für eine Person: Host und Client mit eigenem Token

1. Zwei Tokens erzeugen, eines pro Gerät:

   ```sh
   k-lani-link new-code --out host-token
   k-lani-link new-code --out client-token
   ```

2. In `relay.toml` beide demselben Bereich (`space`) zuordnen, Grenzen für eine Person
   beim ersten Token setzen (Vorlage „Einzelnutzer“, siehe `relay.example.toml`):

   ```toml
   [[tokens]]
   token_file = "host-token"
   space = "ich"
   max_ips_per_day = 8

   [tokens.limits]
   rooms = 4
   sessions = 16
   bytes = 4194304
   pending_puts = 32
   pending_put_bytes = 1048576
   get_waiters = 16
   hello_slots = 16
   requests_per_second = 400
   request_burst = 800
   pair_slots = 4

   [[tokens]]
   token_file = "client-token"
   space = "ich"
   expires = "2027-06-30"
   max_ips_per_day = 12
   ```

   Wichtig: Zusätzliche Schlüssel wie `space` oder `expires` gehören **vor** die Zeile
   `[tokens.limits]` des jeweiligen Tokens. Ein weiteres Token ohne eigenes
   `[tokens.limits]` übernimmt die Grenzen seines Bereichs. `pair_slots = 4` erlaubt
   dem Bereich höchstens vier gleichzeitig offene Kopplungscodes; `pair_slots` unter
   `[global]` begrenzt alle Bereiche zusammen (höchstens 900). Ohne Angabe gelten 4 pro
   Bereich und 64 für das ganze Relay; `pair_slots = 0` schaltet das Koppeln per Code dort ab.

3. Relay neu starten und koppeln. Am einfachsten **per Code**: Am Modellrechner

   ```sh
   k-lani-link offer-https --dir neues-pairing \
     --relay-url https://relay.example.com \
     --admission-token-file host-token \
     --client-admission-token-file client-token
   ```

   ausführen. Das zeigt einen Code wie `472-1234-5678`. Am Laptop im Kontrollfeld unter
   „Enter the pairing code“ eintippen, fertig. Der Code gilt einmal und zehn Minuten; ein
   falsch eingegebener Code ist verbraucht, dann einfach einen neuen erzeugen. Ohne
   Kontrollfeld geht es mit `k-lani-link join-https --dir mein-client --relay-url …`.

   Alternativ das Pairing als Ordner erzeugen und den Client-Ordner sicher übertragen:

   ```sh
   k-lani-link init-https --dir neues-pairing \
     --relay-url https://relay.example.com \
     --admission-token-file host-token \
     --client-admission-token-file client-token
   ```

Geht der Laptop verloren, den Eintrag des Client-Tokens löschen und das Relay neu
starten. Der Host bleibt erreichbar. Danach ein neues Client-Token anlegen und das
Pairing neu erzeugen.

## Die Optionen

| Option | Bedeutung | Gut für |
| --- | --- | --- |
| `space = "name"` | Tokens mit gleichem Namen teilen sich Verbindungen und Grenzen. | ein Token pro Gerät, einzeln sperrbar |
| `expires = "2027-06-30"` | letzter gültiger Tag (UTC), ab Mitternacht danach `401` | Projekte beim Kunden mit festem Ende |
| `allow_ips = ["203.0.113.0/24"]` | nur diese Adressen oder Netze (IPv4 und IPv6) | Host mit fester Adresse, Kundennetz mit festem Ausgang |
| `max_ips_per_day = 8` | höchstens so viele verschiedene Adressen in 24 Stunden (IPv6 je /64); bekannte Adressen bleiben erlaubt | erkennt ein weitergegebenes Token, ohne mobile Nutzung zu verhindern |
| `failed_auth_per_minute = 20` (oben in der Datei) | eine Adresse mit mehr Fehlversuchen pro Minute bekommt `429` für `failed_auth_ban_minutes` (Standard 10) | weniger Last durch Scanner |

Hinweise:

- Ein gültiges Token wird von der Fehlversuchs-Bremse nie gesperrt. Teilen sich Kunde
  und Angreifer eine NAT-Adresse, bleibt der Kunde also nutzbar.
- `max_ips_per_day` großzügig setzen. Ein Rechner mit IPv4 und IPv6 zählt doppelt, und
  ein DSL-Anschluss bekommt oft täglich eine neue Adresse. Für unterwegs lieber 12 statt
  `allow_ips`: Hotel-WLAN und Mobilfunk wechseln die Adresse.
- Adressen werden nur als gesalzene Hashes im Arbeitsspeicher gehalten, höchstens einen
  Tag lang, und nie auf die Platte geschrieben.
- Die Adress-Optionen und die Bremse brauchen den eigenen TLS-Listener des Relays. Hinter
  einem TLS-Proxy (`behind_tls_proxy = true`) sähe das Relay nur die Adresse des Proxys
  und lehnt diese Optionen deshalb beim Start ab.
- Abgelehnt wird immer mit `401`, wie bei einem falschen Token. Die Endpunkte versuchen
  es dann nicht weiter: Der Host beendet sich mit „HTTPS relay rejected access or
  protocol“ und muss neu gestartet werden (als Dienst mit automatischem Neustart geht
  das von selbst). Deshalb beim Host-Token keine knappen Grenzen setzen.
- Ein Tippfehler in `relay.toml` wird beim Start mit Zeilennummer gemeldet, zum Beispiel
  `invalid HTTPS relay configuration (line 14): unknown field 'expirs'`.

Dieselben Optionen gibt es im PHP-Relay. Im WordPress-Plugin stehen sie auf der
Einstellungsseite, siehe [../php-relay/ANLEITUNG.de.md](../php-relay/ANLEITUNG.de.md).
