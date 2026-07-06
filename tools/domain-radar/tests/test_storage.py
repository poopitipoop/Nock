import os
import tempfile
import unittest

from domain_radar.storage import Mention, Storage


class StorageTest(unittest.TestCase):
    def setUp(self):
        fd, self.path = tempfile.mkstemp(suffix=".sqlite3")
        os.close(fd)
        self.storage = Storage(self.path)

    def tearDown(self):
        self.storage.close()
        os.remove(self.path)

    def test_record_mention_creates_term(self):
        m = Mention(platform="reddit", source_url="http://x", snippet="hi", engagement=5, seen_at=100.0)
        self.storage.record_mention("vibe-coding", m, display_term="vibe coding", category="technology_category")

        record = self.storage.get_term("vibe-coding")
        self.assertIsNotNone(record)
        self.assertEqual(record.display_term, "vibe coding")
        self.assertEqual(record.category, "technology_category")

        mentions = self.storage.mentions_for(record.id)
        self.assertEqual(len(mentions), 1)
        self.assertEqual(mentions[0].platform, "reddit")

    def test_repeated_mentions_accumulate_on_same_term(self):
        m1 = Mention(platform="reddit", source_url="a", snippet="one", engagement=1, seen_at=100.0)
        m2 = Mention(platform="youtube", source_url="b", snippet="two", engagement=0, seen_at=200.0)
        self.storage.record_mention("vibe-coding", m1, display_term="vibe coding")
        self.storage.record_mention("vibe-coding", m2, display_term="vibe coding")

        record = self.storage.get_term("vibe-coding")
        mentions = self.storage.mentions_for(record.id)
        self.assertEqual(len(mentions), 2)
        self.assertEqual(record.last_seen, 200.0)
        self.assertEqual(record.first_seen, 100.0)

    def test_candidate_terms_respects_min_mentions_and_digested_flag(self):
        m1 = Mention(platform="reddit", source_url="a", snippet="one", engagement=1, seen_at=100.0)
        m2 = Mention(platform="reddit", source_url="b", snippet="two", engagement=1, seen_at=150.0)
        self.storage.record_mention("term-a", m1, display_term="term a")
        self.storage.record_mention("term-a", m2, display_term="term a")
        self.storage.record_mention("term-b", m1, display_term="term b")

        candidates = self.storage.candidate_terms(min_mentions=2, since=0)
        terms = {c.term for c in candidates}
        self.assertIn("term-a", terms)
        self.assertNotIn("term-b", terms)

        record_a = self.storage.get_term("term-a")
        self.storage.mark_digested(record_a.id)
        candidates_after = self.storage.candidate_terms(min_mentions=2, since=0)
        self.assertEqual(candidates_after, [])


if __name__ == "__main__":
    unittest.main()
