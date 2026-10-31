import importlib.util
import os
from pathlib import Path
import shutil
import tempfile
import tomllib
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("desktop_prepare", Path(__file__).with_name("prepare.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def config():
    return {"signaling_url": "https://signal.example.invalid", "signaling_backend": "ice_exchange",
            "signaling_token": "s" * 43, "room": "r" * 12, "pair_key": "p" * 43, "api_key": "a" * 43,
            "relay": "disabled", "ice_servers": [{"urls": ["stun:stun.example.invalid:3478"]}],
            "pairing_state_dir": "C:/private/state", "management_token_file": "C:/private/operator",
            "model_control_socket": "/old/control.sock", "upstream_api_key": "never-give-this-to-an-agent"}


class PreparationBoundary(unittest.TestCase):
    def test_windows_paths_are_normalized_and_agent_only_receives_inference_key(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(prepare.os, "chown"):
            root = Path(directory)
            source = root / "input.toml"
            source.write_text(prepare.toml(config()))
            output = root / "output"
            output.mkdir()
            prepare.prepare(source, output)
            app = tomllib.loads((output / "app/client.toml").read_text())
            self.assertEqual(app, {"api_key": "a" * 43})
            host = tomllib.loads((output / "link/client.toml").read_text())
            self.assertEqual(host["pairing_state_dir"], "/var/lib/link")
            self.assertEqual(host["upstream_api_key"], "")
            self.assertNotIn("model_control_socket", host)
            self.assertNotIn("management_token_file", host)
            self.assertEqual((output / "link/client.toml").stat().st_mode & 0o777, 0o600)
            # Starting with a new config must never erase consumed-code markers.
            marker = output / "state/consumed.used"
            marker.touch()
            prepare.prepare(source, output)
            self.assertTrue(marker.exists())
            self.assertEqual(set(p.name for p in (output / "app").iterdir()), {"client.toml", "server-password.txt"})

    def test_loopback_rendezvous_turn_and_unknown_fields_are_rejected(self):
        for change in ({"signaling_url": "http://127.0.0.1:28788"},
                       {"signaling_url": "https://localhost"},
                       {"signaling_url": "https://user:secret@example.invalid"},
                       {"signaling_url": "https://example.invalid/?token=secret"},
                       {"relay": "allowed"}, {"ice_servers": []},
                       {"ice_servers": [{"urls": ["turn:example.invalid:3478"]}]},
                       {"unknown_secret": "must-never-be-echoed"}):
            with self.subTest(change=tuple(change)):
                with self.assertRaises(ValueError) as error:
                    prepare.normalize(config() | change)
                self.assertNotIn("must-never-be-echoed", str(error.exception))

    def test_config_is_not_partially_created_for_invalid_input(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(prepare.os, "chown"):
            root = Path(directory)
            source = root / "input.toml"
            source.write_text(prepare.toml(config() | {"relay": "required"}))
            with self.assertRaises(ValueError):
                prepare.prepare(source, root / "output")
            self.assertFalse((root / "output").exists())

    def test_config_symlink_and_oversized_input_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.write_bytes(b"x" * 65537)
            with self.assertRaises(ValueError):
                prepare.prepare(target, root / "output")
            link = root / "link"
            link.symlink_to(target)
            with self.assertRaises(ValueError):
                prepare.prepare(link, root / "output")

    def test_private_file_is_not_world_readable_and_stale_temporary_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(prepare.os, "chown"):
            target = Path(directory) / "client.toml"
            target.with_suffix(".new").write_text("interrupted")
            with self.assertRaises(FileExistsError):
                prepare.private_write(target, "secret")
            self.assertFalse(target.exists())


HTTPS_CLIENT = """transport = "https"
signaling_backend = "mailbox"
signaling_token = ""
room = ""
api_key = "{key}"
listen = "127.0.0.1:8787"
upstream = "http://127.0.0.1:8080"
upstream_api_key = ""
relay = "disabled"
ice_servers = []
request_timeout_seconds = 600
max_concurrent_requests = 1
max_queued_requests = 0

[https]
relays = ["https://relay.example.invalid"]
state_dir = "state"
admission_token_file = "relay-token"
ca_cert_file = "ca.pem"
poll_wait_seconds = 25
"""


def https_folder(root, key="k" * 43, chain=b"chain-v1"):
    """A client folder shaped like `init-https` output."""
    folder = root / ("client-" + key[:4])
    (folder / "state").mkdir(parents=True)
    (folder / "client.toml").write_text(HTTPS_CLIENT.format(key=key))
    (folder / "relay-token").write_text("t" * 43)
    (folder / "ca.pem").write_text("-----BEGIN CERTIFICATE-----\n")
    (folder / "state/client.chain").write_bytes(chain)
    (folder / "state/client.chain.lock").write_bytes(b"")
    return folder


class HttpsImport(unittest.TestCase):
    def test_http_version_switches_pass_through_and_http3_refuses_a_proxy(self):
        base = tomllib.loads(HTTPS_CLIENT.format(key="k" * 43))
        config, _ = prepare.normalize_https(base | {"https": base["https"] | {"http3": True, "http1_only": False}})
        self.assertEqual((config["https"]["http3"], config["https"]["http1_only"]), (True, False))
        with self.assertRaises(ValueError):
            prepare.normalize_https(base | {"https": base["https"] | {"http3": True}}, "http://proxy.example.invalid:3128")
        with self.assertRaises(ValueError):
            prepare.normalize_https(base | {"https": base["https"] | {"http3": "yes"}})

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.output = self.root / "output"
        self.output.mkdir()
        chown = patch.object(prepare.os, "chown")
        chown.start()
        self.addCleanup(chown.stop)
        self.addCleanup(self.directory.cleanup)

    def test_first_start_imports_the_folder_with_container_paths_and_no_agent_secrets(self):
        folder = https_folder(self.root)
        self.assertEqual(prepare.prepare(folder / "client.toml", self.output), "IMPORTED")
        link = tomllib.loads((self.output / "link/client.toml").read_text())
        self.assertEqual(link["transport"], "https")
        self.assertEqual(link["https"], {
            "relays": ["https://relay.example.invalid"], "state_dir": "/var/lib/link/https",
            "admission_token_file": "/run/secrets/relay-token", "ca_cert_file": "/run/secrets/ca.pem",
            "poll_wait_seconds": 25})
        self.assertEqual(link["upstream_api_key"], "")
        self.assertEqual((self.output / "link/relay-token").read_text(), "t" * 43 + "\n")
        self.assertEqual((self.output / "state/https/client.chain").read_bytes(), b"chain-v1")
        self.assertEqual((self.output / "state/https").stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.output / "state/https/client.chain").stat().st_mode & 0o777, 0o600)
        self.assertEqual(tomllib.loads((self.output / "app/client.toml").read_text()), {"api_key": "k" * 43})
        self.assertEqual([p.name for p in (self.output / "state").iterdir()], ["https"])

    def test_a_later_start_keeps_the_advanced_volume_state(self):
        folder = https_folder(self.root)
        prepare.prepare(folder / "client.toml", self.output)
        (self.output / "state/https/client.chain").write_bytes(b"chain-v2-advanced")
        # Even a stale source copy, or none at all, never overwrites the live state.
        self.assertEqual(prepare.prepare(folder / "client.toml", self.output), "KEPT")
        shutil.rmtree(folder / "state")
        self.assertEqual(prepare.prepare(folder / "client.toml", self.output), "KEPT")
        self.assertEqual((self.output / "state/https/client.chain").read_bytes(), b"chain-v2-advanced")

    def test_a_different_pair_is_refused_before_anything_is_written(self):
        prepare.prepare(https_folder(self.root)/ "client.toml", self.output)
        before = (self.output / "link/client.toml").read_bytes()
        other = https_folder(self.root, key="o" * 43, chain=b"other")
        with self.assertRaises(prepare.Refused):
            prepare.prepare(other / "client.toml", self.output)
        self.assertEqual((self.output / "link/client.toml").read_bytes(), before)
        self.assertEqual((self.output / "state/https/client.chain").read_bytes(), b"chain-v1")

    def test_state_without_its_configuration_fails_closed(self):
        folder = https_folder(self.root)
        prepare.prepare(folder / "client.toml", self.output)
        (self.output / "link/client.toml").unlink()
        with self.assertRaises(prepare.Refused):
            prepare.prepare(folder / "client.toml", self.output)

    def test_missing_source_state_on_first_import_is_refused(self):
        folder = https_folder(self.root)
        shutil.rmtree(folder / "state")
        with self.assertRaises(prepare.Refused):
            prepare.prepare(folder / "client.toml", self.output)
        self.assertFalse((self.output / "link").exists())

    def test_a_proxy_for_this_customer_overrides_the_folder(self):
        folder = https_folder(self.root)
        prepare.prepare(folder / "client.toml", self.output, "http://user:pw@proxy.customer.invalid:3128")
        link = tomllib.loads((self.output / "link/client.toml").read_text())
        self.assertEqual(link["https"]["proxy_url"], "http://user:pw@proxy.customer.invalid:3128")
        for proxy in ("socks5://proxy.invalid:1080", "http://127.0.0.1:3128", "http://proxy.invalid/?x=1"):
            with self.subTest(proxy=proxy), self.assertRaises(ValueError):
                prepare.normalize_https(tomllib.loads(HTTPS_CLIENT.format(key="k" * 43)), proxy)

    def test_paths_leaving_the_folder_and_webrtc_leftovers_are_rejected(self):
        base = tomllib.loads(HTTPS_CLIENT.format(key="k" * 43))
        for https in ({"state_dir": "../state"}, {"state_dir": "/var/lib/state"},
                      {"admission_token_file": "C:/token"}, {"ca_cert_file": "certs/../ca.pem"},
                      {"relays": ["http://relay.example.invalid"]}, {"relays": ["https://localhost"]},
                      {"relays": []}, {"unknown": 1}):
            with self.subTest(https=https), self.assertRaises(ValueError):
                prepare.normalize_https(base | {"https": base["https"] | https})
        for change in ({"room": "r" * 12}, {"ice_servers": [{"urls": ["stun:x.invalid"]}]},
                       {"signaling_backend": "ice_exchange"}, {"relay": "allowed"}, {"unknown": "x"}):
            with self.subTest(change=tuple(change)), self.assertRaises(ValueError):
                prepare.normalize_https(base | change)

    def test_symlinks_and_unexpected_state_files_are_rejected(self):
        folder = https_folder(self.root)
        (folder / "relay-token").unlink()
        (folder / "relay-token").symlink_to(self.root / "elsewhere")
        (self.root / "elsewhere").write_text("t" * 43)
        with self.assertRaises(ValueError):
            prepare.prepare(folder / "client.toml", self.output)
        folder = https_folder(self.root, key="s" * 43)
        (folder / "state/extra").write_text("x")
        with self.assertRaises(ValueError):
            prepare.prepare(folder / "client.toml", self.output)
        self.assertFalse((self.output / "state/https").exists())

    def test_only_the_two_known_input_folders_are_read(self):
        with patch.dict(os.environ, {"LINK_INPUT_DIR": "/joined/client"}, clear=False):
            self.assertEqual(prepare.source_path(), Path("/joined/client/client.toml"))
        for folder in ("/joined", "/", "/output/state", "/joined/client/.."):
            with self.subTest(folder=folder), patch.dict(os.environ, {"LINK_INPUT_DIR": folder}), \
                    self.assertRaises(ValueError):
                prepare.source_path()


if __name__ == "__main__":
    unittest.main()
