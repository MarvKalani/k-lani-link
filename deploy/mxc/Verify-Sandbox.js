// Runs inside the same MXC policy as OpenCode using its bundled Bun runtime.
(async () => {
const fs = await import("node:fs");
const path = await import("node:path");
const net = await import("node:net");

const results = [];
function check(name, pass) {
  results.push({ name, pass: Boolean(pass) });
  console.log(`${pass ? "PASS" : "FAIL"} ${name}`);
}
function denied(fn) {
  try { fn(); return false; } catch (e) { return ["EACCES", "EPERM"].includes(e.code); }
}
function connect(host, port) {
  return new Promise(resolve => {
    const socket = net.createConnection({ host, port });
    let done = false;
    function finish(ok) { if (!done) { done = true; socket.destroy(); resolve(ok); } }
    socket.setTimeout(2000, () => finish(false));
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
  });
}
const e = process.env;
const probe = path.join(e.LINK_TEST_PROJECT, `.mxc-check-${crypto.randomUUID()}.txt`);
try {
  fs.writeFileSync(probe, "sandbox-project-ok", { flag: "wx" });
  check("project read/write", fs.readFileSync(probe, "utf8") === "sandbox-project-ok");
} catch { check("project read/write", false); }
finally { try { fs.unlinkSync(probe); } catch {} }
const tool = path.join(e.LINK_TEST_TOOLS, "opencode.exe");
try { const fd = fs.openSync(tool, "r"); fs.closeSync(fd); check("tools readable", true); }
catch { check("tools readable", false); }
const toolProbe = path.join(e.LINK_TEST_TOOLS, `.mxc-check-${crypto.randomUUID()}.txt`);
check("tools not writable", denied(() => fs.writeFileSync(toolProbe, "must-fail", { flag: "wx" })));
try { fs.unlinkSync(toolProbe); } catch {}
check("outside file not readable", denied(() => fs.readFileSync(e.LINK_TEST_OUTSIDE)));
check("outside file not writable", denied(() => fs.appendFileSync(e.LINK_TEST_OUTSIDE, "must-fail")));
const api = new URL(e.LINK_TEST_API);
check("local API TCP reachable", await connect(api.hostname, Number(api.port)));
try {
  const response = await fetch(`${api}/models`, {
    headers: { Authorization: `Bearer ${e.LINK_API_KEY}` }, signal: AbortSignal.timeout(10000)
  });
  const json = await response.json();
  check("authenticated model list", response.ok && Array.isArray(json.data) && json.data.length > 0);
} catch { check("authenticated model list", false); }
check("other live host port blocked", !(await connect("127.0.0.1", Number(e.LINK_TEST_OTHER_PORT))));
check("IPv6 live host port blocked", !(await connect("::1", Number(e.LINK_TEST_OTHER_PORT))));
check("direct internet TCP blocked", !(await connect("1.1.1.1", 443)));
check("direct IPv6 internet TCP blocked", !(await connect("2606:4700:4700::1111", 443)));
try {
  await fetch("https://www.microsoft.com/", { signal: AbortSignal.timeout(5000) });
  check("internet through proxy blocked", false);
} catch { check("internet through proxy blocked", true); }
console.log(JSON.stringify({ checks: results.length, passed: results.filter(r => r.pass).length }));
process.exitCode = results.every(r => r.pass) ? 0 : 1;

})();
