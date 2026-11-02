# Eigener Relay bei Cloudflare (kostenloser Tarif)

Dein eigener k-lani-link-Relay in einem kostenlosen Cloudflare-Konto, ganz ohne Server.
Er sieht nur verschlüsselte Päckchen, hält sie höchstens 30 Sekunden und nur im
Arbeitsspeicher, und er kann nichts lesen oder ändern. Technische Einzelheiten:
[README.md](README.md).

## Einrichten (etwa fünf Minuten)

Du brauchst Node.js ab Version 18 und ein Cloudflare-Konto.

1. Im Ordner `deploy/cloudflare-relay` ein Terminal öffnen und `npm install` ausführen.
2. `npx wrangler login`: Der Browser öffnet sich, du meldest dich bei Cloudflare an.
3. `node tokens.js ~/k-lani-relay-tokens`: Das legt zwei private Zugangsdateien an
   (`host-token`, `client-token`) und zeigt eine Zeile mit `{…}` an.
4. `npx wrangler secret put TOKENS` und genau diese Zeile einfügen. Sie enthält nur
   Prüfsummen der Zugänge, nicht die Zugänge selbst.
5. `npx wrangler deploy`: Am Ende steht die Adresse deines Relays,
   `https://k-lani-relay.<dein-name>.workers.dev`.

Auf dem Modellrechner koppelst du dann wie gewohnt, nur mit dieser Adresse:

```sh
k-lani-link offer-https --dir ~/k-lani-pairing \
  --relay-url https://k-lani-relay.<dein-name>.workers.dev \
  --admission-token-file ~/k-lani-relay-tokens/host-token \
  --client-admission-token-file ~/k-lani-relay-tokens/client-token
```

Im Kontrollfeld auf dem anderen Computer gibst du Relay-Adresse und Code ein. Mit `--qr`
geht es auch per QR-Code.

## Was kostenlos geht

Laut Cloudflare (Stand November 2026, bitte selbst nachsehen): 100.000 Anfragen pro Tag.
Eine verbundene, ruhende Kopplung braucht etwa 7.000 davon am Tag, eine gestreamte
Antwort ein paar Dutzend. Für dich und ein paar Leute reicht das gut.

## Gut zu wissen

- Cloudflare sieht, wann und wie viel übertragen wird, aber nur verschlüsselte Daten.
- Startet Cloudflare den Relay neu (etwa nach einem neuen `deploy`), verbinden sich die
  Geräte von selbst neu; eine laufende Antwort kann dabei abbrechen.
- Der Weg über den Knopf „Deploy to Cloudflare“ in der [README](README.md) ist noch nicht
  ausprobiert; die Befehle oben schon.
