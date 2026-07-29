import { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';

const SCHEMA = `
CREATE TABLE IF NOT EXISTS users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  email         TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  plan          TEXT NOT NULL DEFAULT 'free',
  created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  token      TEXT PRIMARY KEY,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);

-- A product is one thing the retailer sells. It carries their own price so we
-- can say "you are being undercut" rather than just "a number changed".
CREATE TABLE IF NOT EXISTS products (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  my_price   REAL,
  currency   TEXT NOT NULL DEFAULT 'USD',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS products_user ON products(user_id);

-- A watch is one competitor URL for one product.
CREATE TABLE IF NOT EXISTS watches (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  product_id         INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
  competitor         TEXT NOT NULL,
  url                TEXT NOT NULL,
  check_interval_ms  INTEGER,
  last_checked_at    INTEGER,
  last_status        TEXT,
  last_error         TEXT,
  paused             INTEGER NOT NULL DEFAULT 0,
  created_at         INTEGER NOT NULL,
  UNIQUE(product_id, url)
);
CREATE INDEX IF NOT EXISTS watches_due ON watches(paused, last_checked_at);

CREATE TABLE IF NOT EXISTS snapshots (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  watch_id     INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
  price        REAL,
  currency     TEXT,
  in_stock     INTEGER,
  source       TEXT,
  confidence   REAL,
  observed_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS snapshots_watch_time ON snapshots(watch_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS alerts (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  watch_id    INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,
  message     TEXT NOT NULL,
  old_price   REAL,
  new_price   REAL,
  delivered   INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS alerts_user_time ON alerts(user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS robots_cache (
  host       TEXT PRIMARY KEY,
  body       TEXT,
  fetched_at INTEGER NOT NULL
);
`;

export function openDatabase(path) {
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true });
  const db = new DatabaseSync(path);
  db.exec('PRAGMA journal_mode = WAL');
  db.exec('PRAGMA foreign_keys = ON');
  db.exec(SCHEMA);
  return db;
}

// --- users -----------------------------------------------------------------

export function createUser(db, { email, passwordHash, plan = 'free' }) {
  const { lastInsertRowid } = db
    .prepare('INSERT INTO users (email, password_hash, plan, created_at) VALUES (?, ?, ?, ?)')
    .run(email.toLowerCase(), passwordHash, plan, Date.now());
  return getUserById(db, Number(lastInsertRowid));
}

export function getUserByEmail(db, email) {
  return db.prepare('SELECT * FROM users WHERE email = ?').get(email.toLowerCase());
}

export function getUserById(db, id) {
  return db.prepare('SELECT * FROM users WHERE id = ?').get(id);
}

export function setUserPlan(db, userId, plan) {
  db.prepare('UPDATE users SET plan = ? WHERE id = ?').run(plan, userId);
}

// --- sessions --------------------------------------------------------------

export function createSession(db, { token, userId, ttlMs }) {
  const now = Date.now();
  db.prepare(
    'INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)',
  ).run(token, userId, now, now + ttlMs);
}

export function getSessionUser(db, token) {
  if (!token) return undefined;
  return db
    .prepare(
      `SELECT u.* FROM sessions s
       JOIN users u ON u.id = s.user_id
       WHERE s.token = ? AND s.expires_at > ?`,
    )
    .get(token, Date.now());
}

export function deleteSession(db, token) {
  db.prepare('DELETE FROM sessions WHERE token = ?').run(token);
}

export function purgeExpiredSessions(db) {
  return Number(db.prepare('DELETE FROM sessions WHERE expires_at <= ?').run(Date.now()).changes);
}

// --- products and watches --------------------------------------------------

export function countProducts(db, userId) {
  return db.prepare('SELECT COUNT(*) AS c FROM products WHERE user_id = ?').get(userId).c;
}

export function createProduct(db, { userId, name, myPrice, currency = 'USD' }) {
  const { lastInsertRowid } = db
    .prepare(
      'INSERT INTO products (user_id, name, my_price, currency, created_at) VALUES (?, ?, ?, ?, ?)',
    )
    .run(userId, name, myPrice ?? null, currency, Date.now());
  return Number(lastInsertRowid);
}

export function deleteProduct(db, userId, productId) {
  return Number(
    db.prepare('DELETE FROM products WHERE id = ? AND user_id = ?').run(productId, userId).changes,
  );
}

export function listProducts(db, userId) {
  return db.prepare('SELECT * FROM products WHERE user_id = ? ORDER BY created_at DESC').all(userId);
}

export function getProduct(db, userId, productId) {
  return db.prepare('SELECT * FROM products WHERE id = ? AND user_id = ?').get(productId, userId);
}

export function createWatch(db, { productId, competitor, url, checkIntervalMs = null }) {
  const { lastInsertRowid } = db
    .prepare(
      `INSERT INTO watches (product_id, competitor, url, check_interval_ms, created_at)
       VALUES (?, ?, ?, ?, ?)`,
    )
    .run(productId, competitor, url, checkIntervalMs, Date.now());
  return Number(lastInsertRowid);
}

export function deleteWatch(db, userId, watchId) {
  return Number(
    db
      .prepare(
        `DELETE FROM watches WHERE id = ? AND product_id IN
           (SELECT id FROM products WHERE user_id = ?)`,
      )
      .run(watchId, userId).changes,
  );
}

export function listWatches(db, productId) {
  return db.prepare('SELECT * FROM watches WHERE product_id = ? ORDER BY created_at').all(productId);
}

export function getWatchWithOwner(db, watchId) {
  return db
    .prepare(
      `SELECT w.*, p.user_id, p.name AS product_name, p.my_price, p.currency
       FROM watches w JOIN products p ON p.id = w.product_id
       WHERE w.id = ?`,
    )
    .get(watchId);
}

/**
 * Watches that are due a check: never checked, or last checked longer ago than
 * their interval. Paused watches never come due.
 */
export function listDueWatches(db, { now = Date.now(), defaultIntervalMs, limit = 50 }) {
  return db
    .prepare(
      `SELECT w.*, p.user_id, p.name AS product_name, p.my_price, p.currency
       FROM watches w JOIN products p ON p.id = w.product_id
       WHERE w.paused = 0
         AND (w.last_checked_at IS NULL
              OR w.last_checked_at + COALESCE(w.check_interval_ms, ?) <= ?)
       ORDER BY COALESCE(w.last_checked_at, 0) ASC
       LIMIT ?`,
    )
    .all(defaultIntervalMs, now, limit);
}

export function markWatchChecked(db, watchId, { status, error = null, at = Date.now() }) {
  db.prepare('UPDATE watches SET last_checked_at = ?, last_status = ?, last_error = ? WHERE id = ?')
    .run(at, status, error, watchId);
}

export function setWatchPaused(db, userId, watchId, paused) {
  return Number(
    db
      .prepare(
        `UPDATE watches SET paused = ? WHERE id = ? AND product_id IN
           (SELECT id FROM products WHERE user_id = ?)`,
      )
      .run(paused ? 1 : 0, watchId, userId).changes,
  );
}

// --- snapshots -------------------------------------------------------------

export function insertSnapshot(db, { watchId, price, currency, inStock, source, confidence }) {
  db.prepare(
    `INSERT INTO snapshots (watch_id, price, currency, in_stock, source, confidence, observed_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    watchId,
    price ?? null,
    currency ?? null,
    inStock === null || inStock === undefined ? null : inStock ? 1 : 0,
    source ?? null,
    confidence ?? null,
    Date.now(),
  );
}

export function latestSnapshot(db, watchId) {
  return db
    .prepare('SELECT * FROM snapshots WHERE watch_id = ? ORDER BY observed_at DESC LIMIT 1')
    .get(watchId);
}

/** The snapshot before the most recent one, used to describe a change. */
export function previousSnapshot(db, watchId) {
  return db
    .prepare('SELECT * FROM snapshots WHERE watch_id = ? ORDER BY observed_at DESC LIMIT 1 OFFSET 1')
    .get(watchId);
}

export function snapshotHistory(db, watchId, limit = 30) {
  return db
    .prepare('SELECT * FROM snapshots WHERE watch_id = ? ORDER BY observed_at DESC LIMIT ?')
    .all(watchId, limit);
}

// --- alerts ----------------------------------------------------------------

export function insertAlert(db, { userId, watchId, kind, message, oldPrice, newPrice }) {
  const { lastInsertRowid } = db
    .prepare(
      `INSERT INTO alerts (user_id, watch_id, kind, message, old_price, new_price, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    )
    .run(userId, watchId, kind, message, oldPrice ?? null, newPrice ?? null, Date.now());
  return Number(lastInsertRowid);
}

export function markAlertDelivered(db, alertId) {
  db.prepare('UPDATE alerts SET delivered = 1 WHERE id = ?').run(alertId);
}

export function listAlerts(db, userId, limit = 50) {
  return db
    .prepare(
      `SELECT a.*, w.competitor, w.url, p.name AS product_name
       FROM alerts a
       JOIN watches w ON w.id = a.watch_id
       JOIN products p ON p.id = w.product_id
       WHERE a.user_id = ?
       ORDER BY a.created_at DESC LIMIT ?`,
    )
    .all(userId, limit);
}

// --- robots cache ----------------------------------------------------------

export function getCachedRobots(db, host, maxAgeMs) {
  const row = db.prepare('SELECT * FROM robots_cache WHERE host = ?').get(host);
  if (!row) return undefined;
  if (Date.now() - row.fetched_at > maxAgeMs) return undefined;
  return row;
}

export function putCachedRobots(db, host, body) {
  db.prepare(
    `INSERT INTO robots_cache (host, body, fetched_at) VALUES (?, ?, ?)
     ON CONFLICT(host) DO UPDATE SET body = excluded.body, fetched_at = excluded.fetched_at`,
  ).run(host, body, Date.now());
}
