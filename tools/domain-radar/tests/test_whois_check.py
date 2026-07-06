import unittest

from domain_radar.whois_check import check_all_tlds, check_domain


class WhoisCheckTest(unittest.TestCase):
    def test_available_domain_detected(self):
        def fake_query(server, query, timeout):
            return "No match for domain.\n"

        result = check_domain("foobar", "com", query_fn=fake_query)
        self.assertTrue(result)

    def test_taken_domain_detected(self):
        def fake_query(server, query, timeout):
            return "Domain Name: FOOBAR.COM\nRegistrar: Example\n"

        result = check_domain("foobar", "com", query_fn=fake_query)
        self.assertFalse(result)

    def test_query_failure_returns_none(self):
        def fake_query(server, query, timeout):
            raise OSError("connection refused")

        result = check_domain("foobar", "com", query_fn=fake_query)
        self.assertIsNone(result)

    def test_unknown_tld_raises(self):
        with self.assertRaises(ValueError):
            check_domain("foobar", "zzz")

    def test_check_all_tlds_batches(self):
        responses = {
            "com": "Domain Name: FOOBAR.COM\n",
            "io": "No match for FOOBAR.IO\n",
        }

        def fake_query(server, query, timeout):
            tld = query.rsplit(".", 1)[1]
            return responses[tld]

        results = check_all_tlds("foobar", ["com", "io"], query_fn=fake_query, rate_limit_seconds=0)
        self.assertEqual(results, {"foobar.com": False, "foobar.io": True})


if __name__ == "__main__":
    unittest.main()
