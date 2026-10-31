import sys
from pathlib import Path
from unittest import TestCase, main
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex"))
import runtime


class RuntimeBoundary(TestCase):
    def test_network_interface_refuses_start_before_configuration(self):
        with patch.object(runtime.socket, "if_nameindex", return_value=[(1, "lo"), (2, "eth0")]):
            self.assertEqual(runtime.main(), 2)

    def test_swap_refuses_start_before_credentials_are_read(self):
        with patch.object(runtime.socket, "if_nameindex", return_value=[(1, "lo")]), patch.object(
            runtime.Path, "read_text", return_value="Filename Type Size Used Priority\n/swap.img file 100 0 -1\n"
        ):
            self.assertEqual(runtime.main(), 2)

    def test_only_link_is_allowed_and_auth_is_an_environment_reference(self):
        config = runtime.configuration("test-model")
        self.assertEqual(config["providers"]["link"]["env"], ["LINK_API_KEY"])
        self.assertEqual(config["policies"], [
            {"action": "provider.use", "resource": "*", "effect": "deny"},
            {"action": "provider.use", "resource": "link", "effect": "allow"}])
        self.assertEqual(config["share"], "disabled")

    def test_craft_tools_are_opt_in_local_processes_with_wordcraft_roots(self):
        self.assertNotIn("mcp", runtime.configuration("test-model"))
        servers = runtime.configuration("test-model", craft=True)["mcp"]["servers"]
        self.assertEqual(set(servers), {"wordcraft", "gridcraft"})
        self.assertTrue(all(server["type"] == "local" for server in servers.values()))
        self.assertEqual(servers["wordcraft"]["command"][-2:], ["--automation-write-root", "/work/demo/output"])


if __name__ == "__main__":
    main()
