"""Build changed container recipes with disposable inputs for Dependabot's CI job."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
COMPOSE = re.compile(r"(?:^|[.-])compose(?:[.-][\w.-]+)?\.ya?ml$")
PROBE = "#!/bin/sh\nexit 0\n"


def run(*args, **options):
    subprocess.run(args, cwd=ROOT, check=True, **options)


def changed_paths(base, head):
    output = subprocess.check_output(
        ["git", "diff", "--name-only", "--diff-filter=ACMR", "-z", f"{base}...{head}", "--", "deploy"],
        cwd=ROOT,
    )
    return [Path(os.fsdecode(name)) for name in output.split(b"\0") if name]


def verify_compose(path):
    # Expand required bind paths before Compose parses short volume syntax.
    # Use disposable paths so CI needs no deployment configuration.
    variables = re.findall(r"\$\{([A-Z_][A-Z0-9_]*)(?::\?|\?)", (ROOT / path).read_text())
    env = {**os.environ, **dict.fromkeys(variables, "/tmp/k-lani-link-ci")}
    compose = (
        "docker", "compose", "--env-file", os.devnull, "--profile", "*", "-f", str(path), "config",
        "--no-consistency", "--no-path-resolution",
    )
    images = subprocess.check_output([*compose, "--images"], cwd=ROOT, env=env, text=True)
    for image in sorted(set(images.splitlines())):
        if image.startswith(("nginx:", "python:")):
            run("docker", "buildx", "imagetools", "inspect", image, stdout=subprocess.DEVNULL)


def build(file, temp):
    context = ROOT
    args = []
    if file == Path("deploy/basalt.Dockerfile"):
        pin = (ROOT / "integrations/basalt/BASE_COMMIT").read_text().strip()
        args += [
            "--build-context", f"basalt_source=https://github.com/jesdga95/basalt.git#{pin}",
            "--build-arg", "BUILD_JOBS=4",
        ]
    elif file.parent.name in {"mailbox-bench", "proxy-lab"}:
        context = temp / file.parent.name
        shutil.copytree(ROOT / file.parent, context, ignore=shutil.ignore_patterns("target", "__pycache__"))
        binary = context / ("link-mailbox-bench" if file.parent.name == "mailbox-bench" else "k-lani-link")
        binary.write_text(PROBE)
        binary.chmod(0o755)
        file = context / file.name
    elif file == Path("deploy/opencode/craft/Dockerfile"):
        run("docker", "build", "--tag", "k-lani-opencode:2.2.0", "-f", "deploy/opencode/Dockerfile", ".")

    print(f"Building {file}", flush=True)
    run("docker", "buildx", "build", "--output", "type=cacheonly", "-f", str(file), *args, str(context))


def main(base, head):
    paths = changed_paths(base, head)
    compose_files = [path for path in paths if COMPOSE.search(path.name)]
    dockerfiles = [path for path in paths if "dockerfile" in path.name.lower()]
    if not compose_files and not dockerfiles:
        print("No Docker or Compose recipes changed.")
        return

    with tempfile.TemporaryDirectory(prefix="link-docker-ci-") as directory:
        temp = Path(directory)
        for path in compose_files:
            print(f"Validating {path}", flush=True)
            verify_compose(path)
        for file in dockerfiles:
            build(file, temp)


if __name__ == "__main__":
    main(*sys.argv[1:])
