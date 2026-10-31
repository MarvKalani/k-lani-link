import unittest
from metrics import normalize, model_metadata
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


class ResourceAccounting(unittest.TestCase):
    def test_cpu_core_units_cache_subtraction_and_io(self):
        raw = {"cpu_stats": {"cpu_usage": {"total_usage": 300}, "system_cpu_usage": 2000, "online_cpus": 8},
            "memory_stats": {"usage": 1000, "limit": 2000, "stats": {"inactive_file": 400}},
            "blkio_stats": {"io_service_bytes_recursive": [{"op": "read", "value": 42}, {"op": "Write", "value": 5}]},
            "networks": {"eth0": {"rx_bytes": 13, "tx_bytes": 17}}, "pids_stats": {"current": 3}}
        value, previous = normalize(raw, (100, 1000))
        self.assertEqual(value["cpu_percent"], 160)
        self.assertEqual(value["memory_working_set_bytes"], 600)
        self.assertEqual(value["memory_usage_bytes"], 1000)
        self.assertEqual(value["block_read_bytes"], 42)
        self.assertEqual(value["block_write_bytes"], 5)
        self.assertEqual(value["network_rx_bytes"], 13)
        self.assertEqual(value["network_tx_bytes"], 17)
        self.assertEqual(previous, (300, 2000))

    def test_managed_model_metadata_tracks_the_loaded_pack_without_stale_labels(self):
        with TemporaryDirectory() as directory:
            root=Path(directory)
            for name in ("coder", "nvfp4"):(root/(name+".basalt")).write_bytes(name.encode())
            identity={"Config":{"Labels":{"com.k-lani-link.model-preset":"coder"}},"Mounts":[
                {"Destination":"/run/secrets/models.json","Source":str(root/"models.json")},
                *({"Destination":f"/models/{name}.basalt","Source":str(root/(name+".basalt"))} for name in ("coder","nvfp4"))]}
            with patch("metrics.subprocess.check_output",return_value="/dev/md0\n"):
                for name in ("coder", "nvfp4", "coder"):
                    result=model_metadata(identity,{"available":True,"state":"idle","model":name,"context_tokens_per_slot":262144})
                    self.assertEqual(result["preset"],name)
                    self.assertEqual(result["file_name"],name+".basalt")
                result=model_metadata(identity,{"available":False})
                self.assertEqual(result["preset"],"unloaded / loading")
                self.assertNotIn("file_name",result)

    def test_initial_sample_does_not_invent_cpu_usage(self):
        value, _ = normalize({"memory_stats": {"usage": 10, "stats": {"inactive_file": 20}}})
        self.assertIsNone(value["cpu_percent"])
        self.assertEqual(value["memory_working_set_bytes"], 0)


if __name__ == "__main__":
    unittest.main()
