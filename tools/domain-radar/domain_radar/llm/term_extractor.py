"""Uses the LLM to spot candidate emerging terms in a batch of source text."""

from __future__ import annotations

from dataclasses import dataclass

from .client import LLMClient

SYSTEM_PROMPT = """You watch social media and podcast transcripts for the birth of new \
terms that could become valuable domain names: names for a new technology category, \
product pattern, or coined phrase (like "vibe coding" or "prompt engineering" when \
those first appeared). You are NOT looking for established words, generic slang, \
existing well-known product/company names, or one-off jokes with no staying power.

For each snippet, extract zero or more candidate terms. Be conservative: most \
snippets contain no novel coinage at all, and returning an empty list is the \
correct, expected answer most of the time. Only report a term if it plausibly \
reads as a new, nameable category or coinage -- not just any noun phrase."""

TOOL_SCHEMA = {
    "type": "object",
    "properties": {
        "terms": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "term": {"type": "string", "description": "the term as it appears/should be written"},
                    "normalized_slug": {
                        "type": "string",
                        "description": "lowercase, hyphenated form suitable for a domain, e.g. 'vibe-coding'",
                    },
                    "category": {
                        "type": "string",
                        "enum": ["technology_category", "product_pattern", "meme", "slang", "other"],
                    },
                    "est_lifespan": {
                        "type": "string",
                        "enum": ["days", "weeks", "1-3 years", "5+ years"],
                    },
                    "reasoning": {"type": "string", "description": "one sentence, why this looks novel"},
                },
                "required": ["term", "normalized_slug", "category", "est_lifespan", "reasoning"],
            },
        }
    },
    "required": ["terms"],
}


@dataclass
class ExtractedTerm:
    term: str
    normalized_slug: str
    category: str
    est_lifespan: str
    reasoning: str


def extract_terms(client: LLMClient, text: str) -> list[ExtractedTerm]:
    result = client.call_tool(
        system=SYSTEM_PROMPT,
        user=f"Snippet:\n\n{text}",
        tool_name="report_terms",
        tool_description="Report candidate novel/coined terms found in the snippet.",
        input_schema=TOOL_SCHEMA,
    )
    return [ExtractedTerm(**t) for t in result.get("terms", [])]
