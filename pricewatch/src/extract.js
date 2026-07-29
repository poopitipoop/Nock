// Price and availability extraction from structured markup.
//
// Ordered by trustworthiness: JSON-LD is authored for machines and wins, then
// microdata, then social meta tags. Every result carries the source it came
// from so the audit can say *where* it read a value, not just what it read.

import { flattenJsonLd, hasType, jsonLdBlocks, metaTags, findTags } from './html.js';

const IN_STOCK = ['instock', 'instoreonly', 'onlineonly', 'limitedavailability', 'presale'];
const OUT_OF_STOCK = ['outofstock', 'soldout', 'discontinued', 'backorder', 'preorder'];

/**
 * Normalizes a schema.org availability value to a tri-state.
 * Returns true (purchasable), false (not purchasable), or null (unknown).
 *
 * BackOrder and PreOrder are treated as not-currently-purchasable: for price
 * monitoring, "you cannot buy it today" is the useful meaning.
 */
export function parseAvailability(value) {
  if (typeof value !== 'string' || value.trim() === '') return null;
  const token = value.trim().toLowerCase().replace(/^.*[/#]/, '').replace(/[\s_-]/g, '');
  if (IN_STOCK.includes(token)) return true;
  if (OUT_OF_STOCK.includes(token)) return false;
  return null;
}

/**
 * Parses a human-formatted price into a number.
 *
 * The hard case is separator ambiguity: "1,299" is 1299 in the US and "45,50"
 * is 45.5 in much of Europe, and both are a single comma. The rule used here is
 * the common one — when only one separator is present, exactly three trailing
 * digits means it groups thousands, anything else means it is the decimal
 * point. When both separators appear, whichever comes last is the decimal.
 */
export function parsePrice(input) {
  if (typeof input === 'number') return Number.isFinite(input) ? input : null;
  if (typeof input !== 'string') return null;

  const cleaned = input.replace(/[  \s]/g, '').replace(/[^\d.,-]/g, '');
  if (!/\d/.test(cleaned)) return null;

  const negative = cleaned.startsWith('-');
  const digitsAndSeps = cleaned.replace(/-/g, '');

  const lastComma = digitsAndSeps.lastIndexOf(',');
  const lastDot = digitsAndSeps.lastIndexOf('.');

  let normalized;
  if (lastComma !== -1 && lastDot !== -1) {
    const decimalIndex = Math.max(lastComma, lastDot);
    const intPart = digitsAndSeps.slice(0, decimalIndex).replace(/[.,]/g, '');
    const fracPart = digitsAndSeps.slice(decimalIndex + 1).replace(/[.,]/g, '');
    normalized = `${intPart}.${fracPart}`;
  } else if (lastComma !== -1 || lastDot !== -1) {
    const sepIndex = lastComma !== -1 ? lastComma : lastDot;
    const trailing = digitsAndSeps.length - sepIndex - 1;
    const occurrences = digitsAndSeps.split(digitsAndSeps[sepIndex]).length - 1;
    if (trailing === 3 && (occurrences > 1 || sepIndex > 0)) {
      // Grouping separator: "1,299" / "1.299.000"
      normalized = digitsAndSeps.replace(/[.,]/g, '');
    } else {
      normalized = `${digitsAndSeps.slice(0, sepIndex).replace(/[.,]/g, '')}.${digitsAndSeps
        .slice(sepIndex + 1)
        .replace(/[.,]/g, '')}`;
    }
  } else {
    normalized = digitsAndSeps;
  }

  const value = Number.parseFloat(normalized);
  if (!Number.isFinite(value)) return null;
  return negative ? -value : value;
}

function offersOf(node) {
  const raw = node.offers;
  if (!raw) return [];
  return Array.isArray(raw) ? raw : [raw];
}

/**
 * Reads product price and availability from a page, preferring the most
 * machine-authored source available.
 *
 * @returns {{price: number|null, currency: string|null, inStock: boolean|null,
 *            source: string|null, confidence: number}}
 */
export function extractOffer(html) {
  const none = { price: null, currency: null, inStock: null, source: null, confidence: 0 };

  // 1. JSON-LD Product/Offer.
  for (const block of jsonLdBlocks(html)) {
    if (!block.ok) continue;
    for (const node of flattenJsonLd(block.data)) {
      if (!hasType(node, 'Product', 'Offer', 'AggregateOffer')) continue;

      const candidates = hasType(node, 'Product') ? offersOf(node) : [node];
      for (const offer of candidates) {
        if (!offer || typeof offer !== 'object') continue;
        const price = parsePrice(offer.price ?? offer.lowPrice ?? offer.highPrice);
        if (price === null) continue;
        return {
          price,
          currency: offer.priceCurrency ?? node.priceCurrency ?? null,
          inStock: parseAvailability(offer.availability ?? offer.itemCondition ?? ''),
          source: 'json-ld',
          confidence: 0.95,
        };
      }
    }
  }

  // 2. Microdata: itemprop="price", value in `content` or the element text.
  for (const attrs of findTags(html, '[a-z]+')) {
    if ((attrs.itemprop ?? '').toLowerCase() !== 'price') continue;
    const price = parsePrice(attrs.content);
    if (price !== null) {
      return {
        price,
        currency: attrs.itemcurrency ?? null,
        inStock: null,
        source: 'microdata',
        confidence: 0.8,
      };
    }
  }

  // 3. Social/commerce meta tags.
  const meta = metaTags(html);
  for (const key of ['product:price:amount', 'og:price:amount', 'twitter:data1']) {
    const price = parsePrice(meta.get(key));
    if (price !== null) {
      return {
        price,
        currency:
          meta.get('product:price:currency') ?? meta.get('og:price:currency') ?? null,
        inStock: parseAvailability(meta.get('product:availability') ?? ''),
        source: `meta:${key}`,
        confidence: 0.7,
      };
    }
  }

  return none;
}

/** Product-shaped JSON-LD nodes, used to report structured-data completeness. */
export function productNodes(html) {
  const nodes = [];
  for (const block of jsonLdBlocks(html)) {
    if (!block.ok) continue;
    for (const node of flattenJsonLd(block.data)) {
      if (hasType(node, 'Product')) nodes.push(node);
    }
  }
  return nodes;
}
