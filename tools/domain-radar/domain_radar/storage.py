"""SQLite-backed state for tracked terms and their mentions."""

from __future__ import annotations

import sqlite3
import time
from contextlib import closing
from dataclasses import dataclass, field
from typing import Optional

SCHEMA = """
CREATE TABLE IF NOT EXISTS terms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    term TEXT NOT NULL UNIQUE,
    display_term TEXT NOT NULL,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL,
    category TEXT,
    est_lifespan TEXT,
    digested_at REAL
);

CREATE TABLE IF NOT EXISTS mentions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    term_id INTEGER NOT NULL REFERENCES terms(id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    source_url TEXT,
    snippet TEXT,
    engagement INTEGER NOT NULL DEFAULT 0,
    seen_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mentions_term ON mentions(term_id);
"""


@dataclass
class Mention:
    platform: str
    source_url: str
    snippet: str
    engagement: int = 0
    seen_at: float = field(default_factory=time.time)


@dataclass
class TermRecord:
    id: int
    term: str
    display_term: str
    first_seen: float
    last_seen: float
    category: Optional[str]
    est_lifespan: Optional[str]
    digested_at: Optional[float]


class Storage:
    def __init__(self, db_path: str):
        self.db_path = db_path
        self._conn = sqlite3.connect(db_path)
        self._conn.execute("PRAGMA foreign_keys = ON")
        with self._conn:
            self._conn.executescript(SCHEMA)

    def close(self) -> None:
        self._conn.close()

    def __enter__(self) -> "Storage":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def record_mention(self, term: str, mention: Mention, *, display_term: Optional[str] = None,
                        category: Optional[str] = None, est_lifespan: Optional[str] = None) -> int:
        """Upsert a term (keyed on the normalized slug) and attach a new mention to it.
        Returns the term id."""
        now = mention.seen_at
        with self._conn:
            cur = self._conn.execute("SELECT id FROM terms WHERE term = ?", (term,))
            row = cur.fetchone()
            if row is None:
                cur = self._conn.execute(
                    "INSERT INTO terms (term, display_term, first_seen, last_seen, category, est_lifespan) "
                    "VALUES (?, ?, ?, ?, ?, ?)",
                    (term, display_term or term, now, now, category, est_lifespan),
                )
                term_id = cur.lastrowid
            else:
                term_id = row[0]
                self._conn.execute(
                    "UPDATE terms SET last_seen = ?, "
                    "category = COALESCE(?, category), "
                    "est_lifespan = COALESCE(?, est_lifespan) WHERE id = ?",
                    (now, category, est_lifespan, term_id),
                )
            self._conn.execute(
                "INSERT INTO mentions (term_id, platform, source_url, snippet, engagement, seen_at) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (term_id, mention.platform, mention.source_url, mention.snippet,
                 mention.engagement, mention.seen_at),
            )
        return term_id

    def get_term(self, term: str) -> Optional[TermRecord]:
        cur = self._conn.execute(
            "SELECT id, term, display_term, first_seen, last_seen, category, est_lifespan, digested_at "
            "FROM terms WHERE term = ?",
            (term,),
        )
        row = cur.fetchone()
        return TermRecord(*row) if row else None

    def mentions_for(self, term_id: int) -> list[Mention]:
        cur = self._conn.execute(
            "SELECT platform, source_url, snippet, engagement, seen_at "
            "FROM mentions WHERE term_id = ? ORDER BY seen_at ASC",
            (term_id,),
        )
        return [Mention(*row) for row in cur.fetchall()]

    def candidate_terms(self, min_mentions: int, since: float) -> list[TermRecord]:
        """Terms with enough recent mentions that haven't been digested yet."""
        cur = self._conn.execute(
            """
            SELECT t.id, t.term, t.display_term, t.first_seen, t.last_seen, t.category, t.est_lifespan, t.digested_at
            FROM terms t
            WHERE t.digested_at IS NULL
              AND (SELECT COUNT(*) FROM mentions m WHERE m.term_id = t.id) >= ?
              AND t.last_seen >= ?
            """,
            (min_mentions, since),
        )
        return [TermRecord(*row) for row in cur.fetchall()]

    def mark_digested(self, term_id: int, when: Optional[float] = None) -> None:
        with self._conn:
            self._conn.execute(
                "UPDATE terms SET digested_at = ? WHERE id = ?",
                (when if when is not None else time.time(), term_id),
            )
