import unittest

from domain_radar.llm.term_extractor import extract_terms


class FakeClient:
    def __init__(self, response):
        self.response = response
        self.last_call = None

    def call_tool(self, **kwargs):
        self.last_call = kwargs
        return self.response


class TermExtractorTest(unittest.TestCase):
    def test_extracts_terms_from_tool_response(self):
        client = FakeClient({
            "terms": [
                {
                    "term": "vibe coding",
                    "normalized_slug": "vibe-coding",
                    "category": "technology_category",
                    "est_lifespan": "1-3 years",
                    "reasoning": "widely used new coinage",
                }
            ]
        })
        results = extract_terms(client, "some snippet mentioning vibe coding")
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0].normalized_slug, "vibe-coding")
        self.assertEqual(client.last_call["tool_name"], "report_terms")

    def test_empty_terms_list(self):
        client = FakeClient({"terms": []})
        results = extract_terms(client, "nothing interesting here")
        self.assertEqual(results, [])


if __name__ == "__main__":
    unittest.main()
