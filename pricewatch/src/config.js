// Configuration is read from the environment so the same build runs in dev and prod.
// Every value has a usable default except SESSION_SECRET, which must be set in production.

const bool = (v, dflt) => (v === undefined ? dflt : v === '1' || v === 'true');
const int = (v, dflt) => (v === undefined ? dflt : Number.parseInt(v, 10));

export const config = {
  port: int(process.env.PORT, 3000),
  host: process.env.HOST ?? '127.0.0.1',
  databasePath: process.env.DATABASE_PATH ?? 'data/pricewatch.db',
  sessionSecret: process.env.SESSION_SECRET ?? 'dev-only-insecure-secret',
  secureCookies: bool(process.env.SECURE_COOKIES, process.env.NODE_ENV === 'production'),

  // Crawler politeness. These defaults are deliberately conservative: we are
  // reading other people's public pages and should be a rounding error in
  // their traffic, not a load problem.
  userAgent:
    process.env.CRAWLER_USER_AGENT ??
    'PriceWatchBot/0.1 (+https://example.com/bot; price monitoring for retailers)',
  requestTimeoutMs: int(process.env.REQUEST_TIMEOUT_MS, 15000),
  minHostIntervalMs: int(process.env.MIN_HOST_INTERVAL_MS, 5000),
  maxResponseBytes: int(process.env.MAX_RESPONSE_BYTES, 2_000_000),
  respectRobots: bool(process.env.RESPECT_ROBOTS, true),

  // How often the scheduler sweeps for products that are due a check.
  sweepIntervalMs: int(process.env.SWEEP_INTERVAL_MS, 60_000),
  defaultCheckIntervalMs: int(process.env.DEFAULT_CHECK_INTERVAL_MS, 6 * 60 * 60 * 1000),
  schedulerEnabled: bool(process.env.SCHEDULER_ENABLED, true),

  // Outbound alerts. Email needs a provider; see src/alerts.js for the seam.
  alertWebhookUrl: process.env.ALERT_WEBHOOK_URL ?? null,
};

export const PLANS = {
  free: { name: 'Free', maxProducts: 5, priceUsd: 0 },
  pro: { name: 'Pro', maxProducts: 100, priceUsd: 49 },
};

export function planFor(user) {
  return PLANS[user?.plan] ?? PLANS.free;
}

/**
 * Fails fast on configuration that is safe in dev but dangerous in production,
 * so a misconfigured deploy never silently serves insecure sessions.
 */
export function assertProductionConfig(cfg = config, env = process.env.NODE_ENV) {
  if (env !== 'production') return;
  const problems = [];
  if (cfg.sessionSecret === 'dev-only-insecure-secret') {
    problems.push('SESSION_SECRET must be set to a random value in production');
  }
  if (!cfg.secureCookies) {
    problems.push('SECURE_COOKIES should be enabled in production (HTTPS only)');
  }
  if (problems.length > 0) {
    throw new Error(`Refusing to start:\n  - ${problems.join('\n  - ')}`);
  }
}
