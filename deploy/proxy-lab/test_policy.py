"""Negative tests for the lab's boundary, not substitutes for the Docker scenarios."""
import base64
import json
import unittest

from policy import payload_summary, permitted, request_status


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.policy = {"allow": True, "hosts": ["relay.corp.test"], "mode": "allow"}

    def test_exact_domain_and_port_only(self):
        self.assertTrue(permitted(self.policy, "relay.corp.test", 443))
        for host, port in (("relay.corp.test.attacker.test", 443), ("other.corp.test", 443),
                           ("relay.corp.test", 80), ("127.0.0.1", 443)):
            self.assertFalse(permitted(self.policy, host, port))

    def test_missing_or_revoked_policy_denies_every_write_form(self):
        for policy in ({}, {**self.policy, "allow": False}):
            for method, kind in (("PUT", "application/octet-stream"),
                                  ("POST", "application/octet-stream"), ("POST", "application/json")):
                self.assertEqual(request_status(policy, "relay.corp.test", 443, method, "/v1/box/a", kind), 403)

    def test_malformed_policy_and_host_lists_fail_closed(self):
        for policy in (None, [], "allow", {"allow": True, "hosts": "relay.corp.test"},
                       {"allow": True, "hosts": ["relay.corp.test", None]}):
            self.assertFalse(permitted(policy, "relay.corp.test", 443))
            self.assertEqual(request_status(policy, "relay.corp.test", 443, "POST", "/v1/box/a", "application/json"), 403)

    def test_json_compatibility_does_not_allow_unrelated_paths(self):
        policy = {**self.policy, "mode": "json_only"}
        self.assertIsNone(request_status(policy, "relay.corp.test", 443, "POST", "/v1/box/a", "application/json"))
        self.assertEqual(request_status(policy, "relay.corp.test", 443, "POST", "/upload", "application/json"), 403)

    def test_plain_http_is_not_allowed_even_on_port_443(self):
        self.assertEqual(request_status(self.policy, "relay.corp.test", 443, "GET", "/v1/status", "", "http"), 403)

    def test_plaintext_cannot_hide_in_json_base64(self):
        cleartext = b"QUESTION_KLANI_PROXY_LAB_synthetic"
        wrapped = json.dumps({"data": base64.urlsafe_b64encode(cleartext).decode().rstrip("=")}).encode()
        self.assertTrue(payload_summary(wrapped, "application/json")["plaintext_marker"])
        self.assertTrue(payload_summary(b"ANSWER_KLANI_PROXY_LAB_test")["plaintext_marker"])

    def test_evidence_never_contains_original_content(self):
        original = b"a synthetic relay body that must not be logged"
        evidence = payload_summary(original)
        self.assertFalse(evidence["plaintext_marker"])
        self.assertNotIn(original.decode(), json.dumps(evidence))
        self.assertEqual(set(evidence), {"bytes", "record_sha256", "plaintext_marker"})


if __name__ == "__main__":
    unittest.main()
