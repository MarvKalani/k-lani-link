"""Docker lab: the PHP relay as a WordPress plugin behind a web application firewall.

Builds a typical small web host: WordPress (PHP 8.2, mod_php, few Apache workers, 30 s
PHP time limit) with MariaDB, behind OWASP ModSecurity CRS with TLS. Then runs the actual
k-lani-link host and gateway (https-probe, synthetic model) through
https://waf:8443/wp-json/k-lani-link. Everything runs on one internal Docker network and
is removed afterwards. Not a WAN, real-hoster or real-model measurement.
"""
import argparse
import hashlib
import json
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
BINARY = ROOT / "target/x86_64-unknown-linux-musl/release"


def run(*args, check=True, quiet=True, **kwargs):
    result = subprocess.run(args, capture_output=quiet, text=True, **kwargs)
    if check and result.returncode:
        raise RuntimeError(f"command failed: {' '.join(args[:4])} ... {result.stderr[-2000:] if quiet else ''}")
    return result.stdout.strip() if quiet else ""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rate", type=int, default=50)
    parser.add_argument("--tunnels", type=int, default=1)
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--poll-wait", type=int, default=5, help="client's long-poll wait (s)")
    parser.add_argument("--max-wait", type=int, default=5, help="relay's long-poll cap (s)")
    parser.add_argument("--workers", type=int, default=8, help="Apache worker limit of the web host")
    parser.add_argument("--post-writes", action="store_true", help="client sends POST from the start")
    parser.add_argument("--no-waf", action="store_true", help="TLS proxy without ModSecurity rules")
    parser.add_argument("--max-waiters", type=int, default=3, help="relay's cap on waiting reads")
    parser.add_argument("--max-writes", type=int, default=2, help="relay's announced write parallelism")
    parser.add_argument("--fast", action="store_true", help="use the plugin's fast path instead of the REST route")
    parser.add_argument("--keep", action="store_true", help="leave containers running for inspection")
    args = parser.parse_args()

    tag = uuid.uuid4().hex[:8]
    names = {n: f"klwp-{tag}-{n}" for n in ("net", "db", "wp", "waf", "probe")}
    work = Path(tempfile.mkdtemp(prefix=f"klwp-{tag}-"))
    try:
        # TLS for the firewall's public name and a private CA for the endpoints.
        cert = work / "tls"
        cert.mkdir()
        run("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "2",
            "-subj", "/CN=lab CA", "-keyout", str(cert / "ca.key"), "-out", str(cert / "ca.pem"))
        run("openssl", "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-subj", "/CN=waf",
            "-keyout", str(cert / "server.key"), "-out", str(cert / "server.csr"))
        (cert / "ext.txt").write_text("subjectAltName=DNS:waf\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n")
        run("openssl", "x509", "-req", "-in", str(cert / "server.csr"), "-CA", str(cert / "ca.pem"), "-CAkey", str(cert / "ca.key"),
            "-CAcreateserial", "-days", "2", "-extfile", str(cert / "ext.txt"), "-out", str(cert / "server.crt"))
        for path in cert.iterdir():
            path.chmod(0o644)

        # The plugin as it would be uploaded: main file, relay core, uninstall hook.
        plugin = work / "k-lani-link-relay"
        plugin.mkdir()
        shutil.copy(HERE / "wordpress/k-lani-link-relay.php", plugin)
        shutil.copy(HERE / "wordpress/uninstall.php", plugin)
        shutil.copy(HERE / "wordpress/relay.php", plugin)
        shutil.copy(HERE / "k-lani-relay.php", plugin)
        (work / "limits.conf").write_text(
            f"<IfModule mpm_prefork_module>\nStartServers 2\nMinSpareServers 1\nMaxSpareServers {args.workers}\n"
            f"MaxRequestWorkers {args.workers}\nServerLimit {args.workers}\n</IfModule>\n")
        (work / "limits.ini").write_text("max_execution_time = 30\nmemory_limit = 128M\n")

        token = secrets.token_urlsafe(32)
        (work / "token").write_text(token)
        (work / "token").chmod(0o600)
        db_password = secrets.token_urlsafe(18)

        run("docker", "network", "create", "--internal", names["net"])
        run("docker", "run", "-d", "--name", names["db"], "--network", names["net"], "--network-alias", "db",
            "-e", "MARIADB_RANDOM_ROOT_PASSWORD=1", "-e", "MARIADB_DATABASE=wp", "-e", "MARIADB_USER=wp",
            "-e", f"MARIADB_PASSWORD={db_password}", "mariadb:11")
        run("docker", "run", "-d", "--name", names["wp"], "--network", names["net"], "--network-alias", "wordpress",
            "-e", "WORDPRESS_DB_HOST=db", "-e", "WORDPRESS_DB_USER=wp", "-e", f"WORDPRESS_DB_PASSWORD={db_password}",
            "-e", "WORDPRESS_DB_NAME=wp",
            "-v", f"{plugin}:/var/www/html/wp-content/plugins/k-lani-link-relay:ro",
            "-v", f"{work / 'limits.conf'}:/etc/apache2/conf-enabled/zz-limits.conf:ro",
            "-v", f"{work / 'limits.ini'}:/usr/local/etc/php/conf.d/zz-limits.ini:ro",
            "wordpress:php8.2-apache")
        waf = ["docker", "run", "-d", "--name", names["waf"], "--network", names["net"], "--network-alias", "waf",
               "-e", "BACKEND=http://wordpress:80", "-e", "SERVER_NAME=waf",
               "-v", f"{cert / 'server.crt'}:/usr/local/apache2/conf/server.crt:ro",
               "-v", f"{cert / 'server.key'}:/usr/local/apache2/conf/server.key:ro"]
        if args.no_waf:
            waf += ["-e", "MODSEC_RULE_ENGINE=off"]
        run(*waf, "owasp/modsecurity-crs:apache")

        # Install WordPress like a site owner would, with pretty permalinks.
        cli = ["docker", "run", "--rm", "--network", names["net"], "--volumes-from", names["wp"], "--user", "33:33",
               "-e", "WORDPRESS_DB_HOST=db", "-e", "WORDPRESS_DB_USER=wp", "-e", f"WORDPRESS_DB_PASSWORD={db_password}",
               "-e", "WORDPRESS_DB_NAME=wp", "wordpress:cli-php8.2", "wp"]
        deadline = time.monotonic() + 120
        while True:
            installed = subprocess.run(cli + ["core", "install", "--url=https://waf:8443", "--title=Kunde",
                                              "--admin_user=owner", f"--admin_password={secrets.token_urlsafe(18)}",
                                              "--admin_email=owner@example.invalid", "--skip-email"],
                                       capture_output=True, text=True)
            if installed.returncode == 0:
                break
            if time.monotonic() > deadline:
                raise RuntimeError("WordPress installation failed: " + installed.stderr[-500:])
            time.sleep(3)
        run(*cli, "rewrite", "structure", "/%postname%/")
        run(*cli, "plugin", "activate", "k-lani-link-relay")
        run(*cli, "eval", "update_option('k_lani_link_relay', ['tokens' => ['"
            + hashlib.sha256(token.encode()).hexdigest() + "' => ['created' => time()]], 'dir' => '', "
            + f"'max_wait' => {args.max_wait}, 'max_waiters' => {args.max_waiters}, "
            + f"'max_writes' => {args.max_writes}], false);")
        htaccess = run("docker", "exec", names["wp"], "cat", "/var/www/html/.htaccess", check=False)
        if "RewriteRule" not in htaccess:
            raise RuntimeError("WordPress has no rewrite rules; pretty permalinks are required")

        # The actual endpoints, as static binaries in a scratch image.
        image = f"klwp-probe:{tag}"
        context = work / "image"
        context.mkdir()
        shutil.copy2(BINARY / "examples/https-probe", context / "https-probe")
        (context / "Dockerfile").write_text('FROM scratch\nCOPY https-probe /\nUSER 1000:1000\nENTRYPOINT ["/https-probe"]\n')
        run("docker", "build", "-q", "-t", image, str(context))
        state = work / "state"
        state.mkdir()
        state.chmod(0o777)
        probe = ["docker", "run", "--name", names["probe"], "--network", names["net"], "--read-only", "--cap-drop", "ALL",
                 "-v", f"{state}:/state", "-v", f"{work / 'token'}:/token:ro", "-v", f"{cert / 'ca.pem'}:/ca.pem:ro",
                 image, "--relay", "https://waf:8443/wp-content/plugins/k-lani-link-relay/relay.php" if args.fast
                 else "https://waf:8443/wp-json/k-lani-link", "--token-file", "/token", "--ca", "/ca.pem",
                 "--state", "/state", "--out", "/state/result.json", "--rate", str(args.rate), "--tunnels", str(args.tunnels),
                 "--seconds", str(args.seconds), "--poll-wait", str(args.poll_wait)]
        if args.post_writes:
            probe.append("--post-writes")
        started = time.monotonic()
        finished = subprocess.run(probe, capture_output=True, text=True, timeout=args.seconds + 240)
        result = json.loads((state / "result.json").read_text()) if (state / "result.json").exists() else {}
        logs = subprocess.run(["docker", "logs", names["waf"]], capture_output=True, text=True)
        import re
        rules = {}
        for rule in re.findall(r'\[id "([0-9]+)"\]', logs.stdout + logs.stderr):
            rules[rule] = rules.get(rule, 0) + 1
        blocked = "\n".join(f"{count} x rule {rule}" for rule, count in sorted(rules.items(), key=lambda x: -x[1])[:12])
        # Deactivating the plugin must close the fast path at once.
        fast = "http://localhost/wp-content/plugins/k-lani-link-relay/relay.php/v1/hello/AAAAAAAAAAAAAAAAAAAAAA/client"
        status = ["docker", "exec", names["wp"], "curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
                  "-H", f"Authorization: Bearer {token}", fast]
        fast_active = run(*status, check=False)
        run(*cli, "plugin", "deactivate", "k-lani-link-relay")
        fast_deactivated = run(*status, check=False)
        print(json.dumps({"probe_exit": finished.returncode,
                          "fast_path_active": fast_active, "fast_path_after_deactivation": fast_deactivated, "wall_seconds": round(time.monotonic() - started, 1),
                          "settings": vars(args), "result": result,
                          "waf_rules_triggered": blocked.splitlines()}, indent=2))
        if finished.returncode:
            print(finished.stderr[-3000:])
    finally:
        if not args.keep:
            for name in ("probe", "waf", "wp", "db"):
                subprocess.run(["docker", "rm", "-f", "-v", names[name]], capture_output=True)
            subprocess.run(["docker", "network", "rm", names["net"]], capture_output=True)
            subprocess.run(["docker", "image", "rm", f"klwp-probe:{tag}"], capture_output=True)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
