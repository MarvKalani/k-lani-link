import sys
from pathlib import Path
from unittest import TestCase, main
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex"))
import runtime

MODEL = [{"id": "test-model", "context": 262144}]


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
        config = runtime.configuration(MODEL)
        self.assertEqual(config["providers"]["link"]["env"], ["LINK_API_KEY"])
        self.assertEqual(config["policies"], [
            {"action": "provider.use", "resource": "*", "effect": "deny"},
            {"action": "provider.use", "resource": "link", "effect": "allow"}])
        self.assertEqual(config["share"], "disabled")
        self.assertNotIn("reasoning_effort", config["providers"]["link"]["body"])

    def test_craft_tools_are_opt_in_local_processes_with_wordcraft_roots(self):
        self.assertNotIn("mcp", runtime.configuration(MODEL))
        servers = runtime.configuration(MODEL, craft=True)["mcp"]["servers"]
        self.assertEqual(set(servers), {"wordcraft", "gridcraft"})
        self.assertTrue(all(server["type"] == "local" for server in servers.values()))
        self.assertEqual(servers["wordcraft"]["command"][-2:], ["--automation-write-root", "/work/demo/output"])

    def test_static_basalt_and_managed_multi_model_catalogs_keep_their_actual_context(self):
        single = runtime.backend_models({"data": [{"id": "coder", "meta": {"n_ctx": 262144}}]})
        self.assertEqual(single, [{"id": "coder", "context": 262144}])
        models = runtime.backend_models({"data": [
            {"id": "coder", "context_length": 262144, "loaded": False},
            {"id": "Mellum", "context_length": 131072, "loaded": False}]})
        config = runtime.configuration(models)
        offered = config["providers"]["link"]["models"]
        self.assertEqual(offered["basalt"]["modelID"], "coder")
        self.assertEqual(offered["Mellum"]["modelID"], "Mellum")
        self.assertEqual(offered["Mellum"]["limit"]["context"], 131072)
        self.assertEqual(config["model"], "link/basalt")

    def test_context_uses_the_lower_declared_limit_without_an_arbitrary_output_cap(self):
        models = runtime.backend_models({"data": [{"id": "small", "context_length": 8192, "meta": {"n_ctx": 4096}}]})
        limits = runtime.configuration(models)["providers"]["link"]["models"]["basalt"]["limit"]
        self.assertEqual(limits, {"context": 4096, "output": 4096})
        for context in (32768, 131072, 262144):
            with self.subTest(context=context):
                limits = runtime.configuration([{"id": "coder", "context": context}])["providers"]["link"]["models"]["basalt"]["limit"]
                self.assertEqual(limits, {"context": context, "output": context})

    def test_a_loaded_model_is_the_initial_choice_without_hiding_other_models(self):
        models = runtime.backend_models({"data": [
            {"id": "coder", "context_length": 262144, "loaded": False},
            {"id": "Mellum", "context_length": 131072, "loaded": True}]})
        offered = runtime.configuration(models)["providers"]["link"]["models"]
        self.assertEqual(offered["basalt"]["modelID"], "Mellum")
        self.assertEqual(offered["coder"]["modelID"], "coder")

    def test_colliding_display_aliases_do_not_replace_another_model(self):
        models = [{"id": name, "context": 32768} for name in ("first", "basalt", "model-basalt")]
        offered = runtime.configuration(models)["providers"]["link"]["models"]
        self.assertEqual(len(offered), 3)
        self.assertEqual({m["modelID"] for m in offered.values()}, {m["id"] for m in models})

    def test_discovery_rejects_missing_or_unusable_model_metadata(self):
        valid = {"id": "coder", "context_length": 262144}
        for payload in ({}, {"data": []}, {"data": [valid] * 33}, {"data": [valid, valid]},
                {"data": [None]}, {"data": [{"id": "coder"}]},
                *({"data": [{"id": name, "context_length": 262144}]} for name in (None, "", "line\nbreak")),
                *({"data": [{"id": "coder", "context_length": size}]} for size in (True, "262144", 0, 1048577))):
            with self.subTest(payload=payload):
                with self.assertRaises(ValueError):
                    runtime.backend_models(payload)


if __name__ == "__main__":
    main()
