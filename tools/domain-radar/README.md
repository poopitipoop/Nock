# domain-radar

A pipeline that watches Reddit and YouTube for newly-coined terms, uses an
LLM to flag ones that look like they could become a named technology
category or product pattern, checks whether the matching domain names are
still free across a few TLDs, scores the whole thing with a simple 6-signal
confidence model, and sends the results as a daily digest to Telegram.

This is a real, runnable implementation of that idea -- not a mockup. It is
**not** the "podcasts scraped hourly, buyer list auto-generated from
Crunchbase" version some viral threads describe: this one only uses sources
with legitimate, free/official APIs (Reddit's API, YouTube's public RSS
feeds + caption tracks), and it deliberately does **not** ask an LLM to
invent the names of companies that supposedly already use a term -- that's
a good way to end up cold-emailing a real company based on a hallucination.
Instead it hands you pre-built search links so you do that 30-second check
yourself.

## Read this before you run it

- **Domain speculation on generic/coined terms is legal in most
  jurisdictions**, but registering a domain that matches an *existing
  trademark* in bad faith is cybersquatting and can expose you to legal
  action (e.g. the US ACPA) or a UDRP domain-transfer proceeding. Before
  registering anything the pipeline surfaces, check it isn't already
  someone's trademark.
- **WHOIS availability checks here are a strong hint, not ground truth.**
  Response formats vary by registry and the "not found" heuristic can be
  wrong. Always confirm through the registrar before paying for anything.
- **The "buyer profile" / lifespan / category fields are LLM output.**
  Treat them as a first-pass hypothesis to verify, not a fact. Do not
  contact a company based solely on an LLM's guess that they "already use"
  a term -- click through the generated search links and confirm yourself.
- Scraping/automated-query terms of service: Reddit's official API (used
  here via PRAW, read-only) and YouTube's public RSS feeds are the
  supported way to do this. `youtube-transcript-api` reads publicly served
  caption tracks; it's widely used but technically outside YouTube's ToS
  for automated access -- know that going in.

## Architecture

```
Reddit (PRAW)  ---\
                    >--  raw text items  -->  LLM term extraction  -->  SQLite (terms + mentions)
YouTube (RSS +     /                                                        |
 transcripts)  ---/                                                         v
                                                          scoring (6 signals) + WHOIS batch check
                                                                              |
                                                                              v
                                                                  digest text  -->  Telegram
```

- `domain_radar/sources/` -- Reddit and YouTube connectors, each returning
  plain `RawItem`s (platform, text, url, engagement, timestamp).
- `domain_radar/llm/` -- Anthropic client wrapper + the term-extraction
  prompt, forced into structured JSON via tool-use.
- `domain_radar/storage.py` -- SQLite: one row per tracked term, one row
  per mention, so momentum can be computed across runs.
- `domain_radar/scoring.py` -- combines mention velocity, cross-platform
  spread, LLM category weight, engagement, mention breadth, and domain
  availability into a 0-100 confidence score.
- `domain_radar/whois_check.py` -- raw WHOIS socket queries across
  configured TLDs, rate-limited.
- `domain_radar/buyer_links.py` -- pre-built X/Crunchbase/Google search
  links for manual buyer research.
- `domain_radar/digest.py` + `telegram_sender.py` -- formats and sends the
  daily message.
- `domain_radar/pipeline.py` + `main.py` -- glues it all together; run it
  on a schedule (cron, systemd timer, or a scheduled GitHub Actions job).

## Setup

```bash
cd tools/domain-radar
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
cp .env.example .env
```

Fill in `.env`:

| Variable | Where to get it |
|---|---|
| `REDDIT_CLIENT_ID` / `REDDIT_CLIENT_SECRET` | https://www.reddit.com/prefs/apps -> "create app" -> type "script" |
| `REDDIT_USER_AGENT` | any descriptive string, e.g. `domain-radar/0.1 by u/yourname` |
| `ANTHROPIC_API_KEY` | https://console.anthropic.com |
| `TELEGRAM_BOT_TOKEN` | message [@BotFather](https://t.me/BotFather) on Telegram, `/newbot` |
| `TELEGRAM_CHAT_ID` | message your new bot once, then hit `https://api.telegram.org/bot<token>/getUpdates` to read back your chat id |

Edit `config.yaml`:
- `reddit.subreddits` -- the one thing you're expected to tune often.
- `youtube.channels` -- **channel IDs**, not handles. Open the channel
  page, view source, search for `"channelId"`. Replace the placeholder in
  the example config.
- `domains.tlds` -- which TLDs to check per candidate term.
- `scoring.digest_threshold` -- raise it if the digest is too noisy, lower
  it if nothing's clearing the bar.

## Running

```bash
.venv/bin/python main.py -v
```

First run will create `domain_radar.sqlite3` next to `main.py` (configurable
via `DOMAIN_RADAR_DB`). State persists across runs so momentum/velocity
scoring actually means something -- a term needs multiple runs' worth of
mentions before it can clear `scoring.min_mentions` and the confidence
threshold.

### Scheduling

Simplest: a cron entry on any always-on box (a scheduled GitHub Actions
runner works too, but its filesystem doesn't persist between runs unless
you cache `domain_radar.sqlite3` as a build artifact between jobs).

```cron
0 * * * * cd /path/to/tools/domain-radar && .venv/bin/python main.py >> run.log 2>&1
```

## Running the tests

```bash
.venv/bin/pip install pytest
.venv/bin/python -m pytest -q
```

Tests mock all network calls (Reddit, YouTube, WHOIS, LLM, Telegram) --
none of them hit real services.

## Cost

- Reddit API: free.
- YouTube RSS + transcripts: free.
- WHOIS: free (just be polite about rate limits -- see `whois_check.py`).
- Telegram Bot API: free.
- Anthropic API: the only real cost, proportional to how much text you feed
  it per run (one call per source item). Tune `reddit.lookback_hours`,
  subreddit count, and channel count to control volume.
