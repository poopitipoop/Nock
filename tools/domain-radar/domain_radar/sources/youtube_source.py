"""Pulls transcripts of recent videos from configured channels.

Uses each channel's public RSS feed to list recent uploads (no API key needed),
then fetches the auto-generated caption track for each video as a stand-in for
"podcast" monitoring -- most tech podcasts also post video/caption versions.
"""

from __future__ import annotations

import time
from calendar import timegm
from dataclasses import dataclass

import feedparser
from youtube_transcript_api import YouTubeTranscriptApi
from youtube_transcript_api._errors import TranscriptsDisabled, NoTranscriptFound

from ..config import YoutubeConfig

FEED_URL = "https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}"

# Transcripts are chunked to keep individual LLM inputs bounded and to give
# each chunk its own timestamp-ish granularity for mention tracking.
CHUNK_CHARS = 3000


@dataclass
class RawItem:
    platform: str
    text: str
    url: str
    engagement: int
    seen_at: float


def _entry_timestamp(entry) -> float:
    return timegm(entry.published_parsed)


def fetch_recent(cfg: YoutubeConfig) -> list[RawItem]:
    cutoff = time.time() - cfg.lookback_hours * 3600
    items: list[RawItem] = []

    for channel_id in cfg.channels:
        feed = feedparser.parse(FEED_URL.format(channel_id=channel_id))
        for entry in feed.entries:
            published = _entry_timestamp(entry)
            if published < cutoff:
                continue
            video_id = entry.yt_videoid
            url = entry.link
            try:
                transcript = YouTubeTranscriptApi.get_transcript(video_id)
            except (TranscriptsDisabled, NoTranscriptFound):
                continue
            except Exception:
                # Any other transcript-fetch failure shouldn't take down the whole run.
                continue

            full_text = " ".join(seg["text"] for seg in transcript)
            for start in range(0, len(full_text), CHUNK_CHARS):
                chunk = full_text[start:start + CHUNK_CHARS]
                items.append(
                    RawItem(
                        platform="youtube",
                        text=chunk,
                        url=url,
                        engagement=0,
                        seen_at=published,
                    )
                )
    return items
