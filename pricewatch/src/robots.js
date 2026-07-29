// robots.txt parsing, focused on one question: which AI crawlers has this site
// spoken about, and what did it say?
//
// The audit reports these as facts, never as recommendations. Blocking AI
// crawlers is a legitimate, deliberate choice for a lot of publishers, so the
// report tells you what your file currently says and leaves the policy to you.

/** Crawlers whose stated purpose is training, retrieval, or assistant browsing. */
export const AI_CRAWLERS = [
  { agent: 'GPTBot', operator: 'OpenAI', purpose: 'training' },
  { agent: 'OAI-SearchBot', operator: 'OpenAI', purpose: 'search index' },
  { agent: 'ChatGPT-User', operator: 'OpenAI', purpose: 'assistant browsing' },
  { agent: 'ClaudeBot', operator: 'Anthropic', purpose: 'training' },
  { agent: 'Claude-User', operator: 'Anthropic', purpose: 'assistant browsing' },
  { agent: 'Claude-SearchBot', operator: 'Anthropic', purpose: 'search index' },
  { agent: 'Google-Extended', operator: 'Google', purpose: 'training' },
  { agent: 'PerplexityBot', operator: 'Perplexity', purpose: 'search index' },
  { agent: 'Perplexity-User', operator: 'Perplexity', purpose: 'assistant browsing' },
  { agent: 'CCBot', operator: 'Common Crawl', purpose: 'open crawl corpus' },
  { agent: 'Applebot-Extended', operator: 'Apple', purpose: 'training' },
  { agent: 'meta-externalagent', operator: 'Meta', purpose: 'training' },
  { agent: 'Bytespider', operator: 'ByteDance', purpose: 'training' },
];

/**
 * Parses robots.txt into user-agent groups.
 *
 * Consecutive `User-agent` lines share one rule block, per the spec, so
 * `User-agent: a` followed by `User-agent: b` then `Disallow: /` disallows both.
 */
export function parseRobots(text) {
  const groups = [];
  let current = null;
  let lastLineWasAgent = false;

  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const line = rawLine.replace(/#.*$/, '').trim();
    if (!line) continue;
    const colon = line.indexOf(':');
    if (colon === -1) continue;

    const field = line.slice(0, colon).trim().toLowerCase();
    const value = line.slice(colon + 1).trim();

    if (field === 'user-agent') {
      if (!current || !lastLineWasAgent) {
        current = { agents: [], rules: [] };
        groups.push(current);
      }
      current.agents.push(value.toLowerCase());
      lastLineWasAgent = true;
      continue;
    }

    lastLineWasAgent = false;
    if (!current) continue;
    if (field === 'allow' || field === 'disallow') {
      current.rules.push({ type: field, path: value });
    }
  }

  return groups;
}

/** The group that applies to `agent`, falling back to the wildcard group. */
function groupFor(groups, agent) {
  const wanted = agent.toLowerCase();
  const exact = groups.find((g) => g.agents.includes(wanted));
  if (exact) return exact;
  return groups.find((g) => g.agents.includes('*'));
}

/**
 * Longest-match evaluation of a path against a group's rules, which is how
 * real crawlers resolve conflicting Allow/Disallow lines. An empty Disallow
 * value means "nothing is disallowed".
 */
export function isAllowed(groups, agent, path = '/') {
  const group = groupFor(groups, agent);
  if (!group) return { allowed: true, reason: 'no matching group' };

  let best = null;
  for (const rule of group.rules) {
    if (rule.type === 'disallow' && rule.path === '') continue;
    if (rule.path === '' || path.startsWith(rule.path)) {
      if (best === null || rule.path.length > best.path.length) best = rule;
    }
  }

  if (!best) return { allowed: true, reason: 'no rule matched', explicit: group.agents.includes(agent.toLowerCase()) };
  return {
    allowed: best.type === 'allow',
    reason: `${best.type}: ${best.path || '(empty)'}`,
    explicit: group.agents.includes(agent.toLowerCase()),
  };
}

/** Per-crawler verdicts for the report table. */
export function aiCrawlerStatus(robotsText, path = '/') {
  const groups = parseRobots(robotsText);
  return AI_CRAWLERS.map((crawler) => {
    const verdict = isAllowed(groups, crawler.agent, path);
    return {
      ...crawler,
      allowed: verdict.allowed,
      explicit: Boolean(verdict.explicit),
      reason: verdict.reason,
    };
  });
}
