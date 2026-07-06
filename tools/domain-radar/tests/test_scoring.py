import unittest

from domain_radar.scoring import score_term
from domain_radar.storage import Mention

NOW = 1_000_000.0
DAY = 86400.0


class ScoringTest(unittest.TestCase):
    def test_strong_signal_scores_high(self):
        mentions = [
            Mention(platform="reddit", source_url="a", snippet="", engagement=100, seen_at=NOW - 1 * 3600),
            Mention(platform="reddit", source_url="b", snippet="", engagement=80, seen_at=NOW - 2 * 3600),
            Mention(platform="youtube", source_url="c", snippet="", engagement=0, seen_at=NOW - 3 * 3600),
            Mention(platform="reddit", source_url="d", snippet="", engagement=50, seen_at=NOW - 5 * 3600),
        ]
        availability = {"foo.com": False, "foo.io": True, "foo.ai": True}
        breakdown = score_term(mentions, "technology_category", availability, now=NOW)
        self.assertGreater(breakdown.total, 60)

    def test_weak_signal_scores_low(self):
        mentions = [
            Mention(platform="reddit", source_url="a", snippet="", engagement=1, seen_at=NOW - 10 * DAY),
        ]
        availability = {"foo.com": False, "foo.io": False, "foo.ai": False}
        breakdown = score_term(mentions, "meme", availability, now=NOW)
        self.assertLess(breakdown.total, 30)

    def test_domain_opportunity_zero_when_all_taken(self):
        mentions = [Mention(platform="reddit", source_url="a", snippet="", engagement=10, seen_at=NOW)]
        availability = {"foo.com": False, "foo.io": False}
        breakdown = score_term(mentions, "technology_category", availability, now=NOW)
        self.assertEqual(breakdown.domain_opportunity, 0.0)

    def test_domain_opportunity_neutral_when_unknown(self):
        mentions = [Mention(platform="reddit", source_url="a", snippet="", engagement=10, seen_at=NOW)]
        availability = {"foo.com": None, "foo.io": None}
        breakdown = score_term(mentions, "technology_category", availability, now=NOW)
        self.assertEqual(breakdown.domain_opportunity, 7.5)

    def test_total_is_clamped_to_100(self):
        mentions = [
            Mention(platform="reddit", source_url=f"u{i}", snippet="", engagement=1000, seen_at=NOW - 3600)
            for i in range(20)
        ] + [
            Mention(platform="youtube", source_url=f"y{i}", snippet="", engagement=1000, seen_at=NOW - 3600)
            for i in range(20)
        ]
        availability = {"foo.com": True, "foo.io": True, "foo.ai": True}
        breakdown = score_term(mentions, "technology_category", availability, now=NOW)
        self.assertEqual(breakdown.total, 100)


if __name__ == "__main__":
    unittest.main()
