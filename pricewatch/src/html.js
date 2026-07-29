// Zero-dependency HTML inspection.
//
// This is deliberately not a real parser. We only need to answer a fixed set of
// questions about a document ("is there a title", "what JSON-LD is present",
// "how much text survives with scripts removed"), and a regex pass over the
// source is both adequate and fast for that. Anywhere the imprecision could
// mislead a user, the audit reports what it observed rather than asserting a
// conclusion.

const NAMED_ENTITIES = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' ',
  '#39': "'",
};

export function decodeEntities(text) {
  if (!text) return '';
  return text.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (match, entity) => {
    if (entity[0] === '#') {
      const isHex = entity[1] === 'x' || entity[1] === 'X';
      const code = Number.parseInt(isHex ? entity.slice(2) : entity.slice(1), isHex ? 16 : 10);
      if (!Number.isFinite(code) || code < 0 || code > 0x10ffff) return match;
      try {
        return String.fromCodePoint(code);
      } catch {
        return match;
      }
    }
    const named = NAMED_ENTITIES[entity.toLowerCase()];
    return named ?? match;
  });
}

/** Parses the attributes of a single tag, e.g. `<meta name="x" content="y">`. */
export function parseAttributes(tag) {
  const attrs = {};
  const re = /([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*(?:=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'`=<>]+)))?/g;
  // Skip the tag name itself.
  const withoutName = tag.replace(/^<\s*\/?\s*[a-zA-Z0-9-]+/, '');
  let match;
  while ((match = re.exec(withoutName)) !== null) {
    const name = match[1].toLowerCase();
    const value = match[2] ?? match[3] ?? match[4] ?? '';
    if (!(name in attrs)) attrs[name] = decodeEntities(value);
  }
  return attrs;
}

/**
 * Text a client that does not execute JavaScript would see. Elements whose
 * contents are not rendered prose (script, style, svg, template) are dropped
 * entirely rather than flattened, so their contents cannot inflate the count.
 */
export function visibleText(html) {
  const stripped = html
    .replace(/<!--[\s\S]*?-->/g, ' ')
    .replace(/<script\b[^>]*>[\s\S]*?<\/script\s*>/gi, ' ')
    .replace(/<style\b[^>]*>[\s\S]*?<\/style\s*>/gi, ' ')
    .replace(/<svg\b[^>]*>[\s\S]*?<\/svg\s*>/gi, ' ')
    .replace(/<template\b[^>]*>[\s\S]*?<\/template\s*>/gi, ' ')
    .replace(/<[^>]+>/g, ' ');
  return decodeEntities(stripped).replace(/\s+/g, ' ').trim();
}

export function findTags(html, tagName) {
  const re = new RegExp(`<${tagName}\\b[^>]*>`, 'gi');
  return (html.match(re) ?? []).map(parseAttributes);
}

export function firstTagContent(html, tagName) {
  const re = new RegExp(`<${tagName}\\b[^>]*>([\\s\\S]*?)<\\/${tagName}\\s*>`, 'i');
  const match = html.match(re);
  if (!match) return null;
  return decodeEntities(match[1].replace(/<[^>]+>/g, ' ')).replace(/\s+/g, ' ').trim();
}

/**
 * Collects meta tags keyed by their `name`, `property`, or `http-equiv`, since
 * different vocabularies (Open Graph, Twitter, plain HTML) use different ones.
 */
export function metaTags(html) {
  const out = new Map();
  for (const attrs of findTags(html, 'meta')) {
    const key = (attrs.property ?? attrs.name ?? attrs['http-equiv'] ?? '').toLowerCase();
    if (!key) continue;
    if (!out.has(key)) out.set(key, attrs.content ?? '');
  }
  return out;
}

export function countTags(html, tagName) {
  const re = new RegExp(`<${tagName}\\b[^>]*>`, 'gi');
  return (html.match(re) ?? []).length;
}

export function htmlLang(html) {
  const match = html.match(/<html\b[^>]*>/i);
  if (!match) return null;
  const lang = parseAttributes(match[0]).lang;
  return lang ? lang.trim() : null;
}

export function canonicalHref(html) {
  for (const attrs of findTags(html, 'link')) {
    const rel = (attrs.rel ?? '').toLowerCase().split(/\s+/);
    if (rel.includes('canonical') && attrs.href) return attrs.href.trim();
  }
  return null;
}

export function imageAltCoverage(html) {
  const imgs = findTags(html, 'img');
  let withAlt = 0;
  for (const attrs of imgs) {
    // A present-but-empty alt is a valid choice for decorative images, so it
    // counts as handled rather than missing.
    if (attrs.alt !== undefined) withAlt += 1;
  }
  return { total: imgs.length, withAlt };
}

/**
 * Extracts every JSON-LD block, reporting parse failures rather than throwing:
 * a broken block is one of the more useful things the audit can surface.
 */
export function jsonLdBlocks(html) {
  const re =
    /<script\b[^>]*type\s*=\s*["']application\/ld\+json["'][^>]*>([\s\S]*?)<\/script\s*>/gi;
  const blocks = [];
  let match;
  while ((match = re.exec(html)) !== null) {
    const raw = match[1].trim();
    if (!raw) continue;
    try {
      blocks.push({ ok: true, data: JSON.parse(raw) });
    } catch (error) {
      blocks.push({ ok: false, error: error.message, raw: raw.slice(0, 200) });
    }
  }
  return blocks;
}

/** Flattens JSON-LD (arrays, `@graph`, nested offers) into a list of objects. */
export function flattenJsonLd(value, out = []) {
  if (Array.isArray(value)) {
    for (const item of value) flattenJsonLd(item, out);
    return out;
  }
  if (value && typeof value === 'object') {
    out.push(value);
    for (const key of Object.keys(value)) {
      if (key === '@context') continue;
      flattenJsonLd(value[key], out);
    }
  }
  return out;
}

export function typesOf(node) {
  const raw = node?.['@type'];
  if (!raw) return [];
  return (Array.isArray(raw) ? raw : [raw]).filter((t) => typeof t === 'string');
}

export function hasType(node, ...wanted) {
  const types = typesOf(node).map((t) => t.toLowerCase().replace(/^.*\//, ''));
  return wanted.some((w) => types.includes(w.toLowerCase()));
}
