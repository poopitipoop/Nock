"""Wires sources -> LLM extraction -> storage -> scoring -> WHOIS -> digest -> Telegram."""

from __future__ import annotations

import logging
import time

from .buyer_links import build_buyer_lead_links
from .config import Config
from .digest import DigestSignal, format_digest
from .llm.client import LLMClient
from .llm.term_extractor import extract_terms
from .scoring import score_term
from .sources import reddit_source, youtube_source
from .storage import Mention, Storage
from .telegram_sender import send_digest
from .whois_check import check_all_tlds

log = logging.getLogger("domain_radar")

# how far back a term can have been first seen and still be eligible for
# today's digest -- keeps a term from lingering forever if it never quite
# clears the threshold.
CANDIDATE_WINDOW_HOURS = 7 * 24


def ingest(cfg: Config, storage: Storage, llm: LLMClient) -> None:
    raw_items = []
    try:
        raw_items.extend(reddit_source.fetch_recent(cfg.reddit))
    except Exception:
        log.exception("reddit fetch failed, continuing without it")
    try:
        raw_items.extend(youtube_source.fetch_recent(cfg.youtube))
    except Exception:
        log.exception("youtube fetch failed, continuing without it")

    log.info("fetched %d raw items", len(raw_items))

    for item in raw_items:
        try:
            terms = extract_terms(llm, item.text)
        except Exception:
            log.exception("term extraction failed for one item, skipping it")
            continue
        for extracted in terms:
            mention = Mention(
                platform=item.platform,
                source_url=item.url,
                snippet=item.text[:500],
                engagement=item.engagement,
                seen_at=item.seen_at,
            )
            storage.record_mention(
                extracted.normalized_slug,
                mention,
                display_term=extracted.term,
                category=extracted.category,
                est_lifespan=extracted.est_lifespan,
            )


def build_digest_signals(cfg: Config, storage: Storage) -> list[DigestSignal]:
    now = time.time()
    since = now - CANDIDATE_WINDOW_HOURS * 3600
    candidates = storage.candidate_terms(cfg.scoring.min_mentions, since)

    signals = []
    for record in candidates:
        mentions = storage.mentions_for(record.id)
        availability = check_all_tlds(record.term, cfg.domains.tlds)
        breakdown = score_term(mentions, record.category, availability, now=now)
        if breakdown.total < cfg.scoring.digest_threshold:
            continue
        signals.append((record, DigestSignal(
            term=record.display_term,
            category=record.category,
            est_lifespan=record.est_lifespan,
            availability=availability,
            score=breakdown,
            mentions=mentions,
            buyer_links=build_buyer_lead_links(record.display_term),
        )))

    signals.sort(key=lambda pair: pair[1].score.total, reverse=True)
    top = signals[:cfg.digest.max_signals_per_run]

    for record, _ in top:
        storage.mark_digested(record.id)

    return [sig for _, sig in top]


def run_once(cfg: Config) -> str:
    llm = LLMClient(cfg.anthropic_api_key, cfg.anthropic_model)
    with Storage(cfg.db_path) as storage:
        ingest(cfg, storage, llm)
        signals = build_digest_signals(cfg, storage)
        text = format_digest(signals)

    if cfg.digest.telegram_bot_token and cfg.digest.telegram_chat_id:
        send_digest(cfg.digest.telegram_bot_token, cfg.digest.telegram_chat_id, text)
    else:
        log.warning("Telegram not configured -- printing digest instead of sending it")

    return text
