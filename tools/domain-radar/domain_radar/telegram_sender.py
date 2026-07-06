"""Sends the digest to Telegram, chunked to stay under the 4096-char message limit."""

from __future__ import annotations

import requests

API_URL = "https://api.telegram.org/bot{token}/sendMessage"
MAX_LEN = 4000  # leave headroom under Telegram's 4096 hard limit


def _chunks(text: str, max_len: int = MAX_LEN) -> list[str]:
    parts = text.split("\n\n")
    chunks: list[str] = []
    current = ""
    for part in parts:
        candidate = f"{current}\n\n{part}" if current else part
        if len(candidate) > max_len and current:
            chunks.append(current)
            current = part
        else:
            current = candidate
    if current:
        chunks.append(current)
    return chunks or [text[:max_len]]


def send_digest(bot_token: str, chat_id: str, text: str, *, session: requests.Session | None = None) -> None:
    if not bot_token or not chat_id:
        raise ValueError("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must both be set")
    http = session or requests
    url = API_URL.format(token=bot_token)
    for chunk in _chunks(text):
        response = http.post(url, data={"chat_id": chat_id, "text": chunk}, timeout=15)
        response.raise_for_status()
