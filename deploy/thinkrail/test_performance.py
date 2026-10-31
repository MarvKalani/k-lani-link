import unittest
from performance import sanitize


class PerformanceTests(unittest.TestCase):
    def snapshot(self):
        return {"schema": 1, "backend": "basalt", "state": "generating", "observed_at": 100,
                "process_started_at": 90, "model": "example", "live_decode_tokens_per_second": 12.5,
                "last": {"output_tokens": 128, "prompt": "SECRET"}, "prompt": "SECRET"}

    def test_only_allowlisted_metrics_are_exported(self):
        result = sanitize(self.snapshot(), 101, 89)
        self.assertTrue(result["available"])
        self.assertEqual(result["live_decode_tokens_per_second"], 12.5)
        self.assertNotIn("SECRET", repr(result))
        self.assertIsNone(result["live_prefill_tokens_per_second"])

    def test_stale_previous_process_and_future_metrics_are_unavailable(self):
        for now, started in ((106, 89), (101, 95), (97, 89)):
            self.assertFalse(sanitize(self.snapshot(), now, started)["available"])

    def test_nonfinite_negative_and_boolean_rates_are_not_numbers(self):
        for value in (float("nan"), float("inf"), -1, True, "20"):
            raw = self.snapshot()
            raw["live_decode_tokens_per_second"] = value
            self.assertIsNone(sanitize(raw, 101, 89)["live_decode_tokens_per_second"])


if __name__ == "__main__":
    unittest.main()
