"""Scripted model for the Codex integration test; the actual Basalt serving path is used."""
import os
from pathlib import Path
import sys
import threading

sink = os.open(os.devnull, os.O_WRONLY)
os.dup2(sink, 1)
os.dup2(sink, 2)
os.close(sink)
sys.path.insert(0, "/opt/basalt")
from serve.engine.mock import MockEngine
from serve.frontend import ChatTemplate
from serve.handler import serve
from serve.service import Service
from serve.util.text import ByteTokenizer

seed = Path("/test/seed.txt").read_text().strip()
patch = "*** Begin Patch\n*** Add File: result.txt\n+" + seed + "\n+verified\n*** End Patch"


def call(name, key, value):
    return f"Checking the local workspace.</think>\n\n<tool_call>\n<function={name}>\n<parameter={key}>\n{value}\n</parameter>\n</function>\n</tool_call>"


tok = ByteTokenizer()
engine = MockEngine(tok, [call("exec_command", "cmd", "cat seed.txt"), call("apply_patch", "input", patch),
    call("exec_command", "cmd", "cat result.txt"), "Verified the result.</think>\n\nlink-agent-ok"],
    max_context=131072, delay_s=0.001, record_requests=False)
svc = Service(engine, tok, ChatTemplate("/opt/basalt/serve/chat_template.jinja"), privacy=True)
server = serve(svc, unix_socket="/run/link/basalt.sock")
threading.Event().wait()
