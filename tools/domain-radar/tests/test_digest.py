import unittest

from domain_radar.buyer_links import build_buyer_lead_links
from domain_radar.digest import DigestSignal, format_digest
from domain_radar.scoring import ScoreBreakdown
from domain_radar.storage import Mention


class DigestTest(unittest.TestCase):
    def test_empty_signals(self):
        self.assertIn("No signals", format_digest([]))

    def test_formats_signal_with_expected_fields(self):
        signal = DigestSignal(
            term="vibe coding",
            category="technology_category",
            est_lifespan="1-3 years",
            availability={"vibe-coding.com": False, "vibe-coding.io": True},
            score=ScoreBreakdown(20, 15, 20, 10, 10, 15),
            mentions=[Mention(platform="reddit", source_url="http://x", snippet="s", engagement=5, seen_at=0)],
            buyer_links=build_buyer_lead_links("vibe coding"),
        )
        text = format_digest([signal])
        self.assertIn('Signal #1: "vibe coding"', text)
        self.assertIn("vibe-coding.io: free", text)
        self.assertIn("vibe-coding.com: taken", text)
        self.assertIn("Confidence: 90/100", text)
        self.assertIn("Register vibe-coding.io", text)

    def test_recommendation_when_nothing_free(self):
        signal = DigestSignal(
            term="taken term",
            category="meme",
            est_lifespan="days",
            availability={"taken-term.com": False},
            score=ScoreBreakdown(0, 0, 0, 0, 0, 0),
            mentions=[],
            buyer_links=build_buyer_lead_links("taken term"),
        )
        text = format_digest([signal])
        self.assertIn("already closed", text)


if __name__ == "__main__":
    unittest.main()
