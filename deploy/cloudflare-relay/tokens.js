// Creates a host and a client admission token for this relay and prints the value of the
// `TOKENS` secret. The token files are private; the secret holds only their SHA-256 hashes.
//
//   node tokens.js /path/to/private-folder [space]
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [folder, space = "me"] = process.argv.slice(2);
if (!folder) {
  console.error("usage: node tokens.js /path/to/private-folder [space]");
  process.exit(2);
}
mkdirSync(folder, { recursive: true, mode: 0o700 });
const secret = {};
for (const name of ["host-token", "client-token"]) {
  const file = join(folder, name);
  if (existsSync(file)) {
    console.error(`${file} exists; refusing to overwrite it`);
    process.exit(1);
  }
  const token = randomBytes(32).toString("base64url");
  writeFileSync(file, token, { mode: 0o600, flag: "wx" });
  secret[createHash("sha256").update(token).digest("hex")] = { space };
}
console.log("Token files written to " + folder);
console.log("Value for `npx wrangler secret put TOKENS` (contains only hashes):");
console.log(JSON.stringify(secret));
