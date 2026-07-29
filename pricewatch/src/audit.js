// The audit itself: given a fetched page, report what a non-executing client
// actually sees.
//
// Design rule for every check in this file: state observations, not promises.
// We can say "your product markup omits a price" because that is verifiable
// from the document. We cannot say "adding it will make assistants recommend
// you", so nothing here claims that. Each finding carries a `fact` (what we
// saw) separate from its `fix` (what you could do about it), and severity
// reflects how confident we are that the observation matters.

import {
  canonicalHref,
  countTags,
  firstTagContent,
  htmlLang,
  imageAltCoverage,
  jsonLdBlocks,
  metaTags,
  visibleText,
} from './html.js';
import { extractOffer, productNodes } from './extract.js';
import { aiCrawlerStatus } from './robots.js';

export const SEVERITY = { high: 'high', medium: 'medium', low: 'low', info: 'info' };

/** Weights sum to 100. Content visibility dominates because it gates everything else. */
const WEIGHTS = {
  contentWithoutJs: 30,
  title: 8,
  description: 7,
  structuredDataValid: 15,
  headings: 8,
  landmarks: 6,
  lang: 4,
  canonical: 6,
  openGraph: 6,
  imageAlt: 5,
  llmsTxt: 5,
};

function check(id, label, weight, earned, { severity, fact, fix = null, detail = null }) {
  return { id, label, weight, earned, ratio: weight === 0 ? 1 : earned / weight, severity, fact, fix, detail };
}

/**
 * A page whose text only appears after JavaScript runs is invisible to clients
 * that fetch and read without executing. This is the single most consequential
 * thing the scan can tell someone, so it carries the largest weight.
 */
function checkContentWithoutJs(html) {
  const text = visibleText(html);
  const words = text ? text.split(/\s+/).length : 0;
  const scriptCount = countTags(html, 'script');
  const w = WEIGHTS.contentWithoutJs;

  if (words >= 250) {
    return check('content-without-js', 'Content visible without JavaScript', w, w, {
      severity: SEVERITY.info,
      fact: `${words} words of text are present in the raw HTML.`,
      detail: { words, scriptCount },
    });
  }
  if (words >= 50) {
    return check('content-without-js', 'Content visible without JavaScript', w, w * 0.5, {
      severity: SEVERITY.medium,
      fact: `Only ${words} words of text are in the raw HTML, across ${scriptCount} script tags.`,
      fix: 'Some content appears to be assembled client-side. Server-render or pre-render the main content so a client that does not execute JavaScript still receives it.',
      detail: { words, scriptCount },
    });
  }
  return check('content-without-js', 'Content visible without JavaScript', w, 0, {
    severity: SEVERITY.high,
    fact: `The raw HTML contains ${words} words of text but ${scriptCount} script tags.`,
    fix: 'This page appears to render entirely client-side. A client that fetches without executing JavaScript sees almost nothing. Server-side rendering or pre-rendering is the fix.',
    detail: { words, scriptCount },
  });
}

function checkTitle(html) {
  const title = firstTagContent(html, 'title');
  const w = WEIGHTS.title;
  if (!title) {
    return check('title', 'Page title', w, 0, {
      severity: SEVERITY.high,
      fact: 'No <title> element was found.',
      fix: 'Add a descriptive <title>. It is the most commonly quoted single field on a page.',
    });
  }
  if (title.length < 10 || title.length > 70) {
    return check('title', 'Page title', w, w * 0.5, {
      severity: SEVERITY.low,
      fact: `Title is ${title.length} characters: "${title}".`,
      fix: 'Aim for roughly 10-70 characters so it is descriptive without being truncated.',
      detail: { title },
    });
  }
  return check('title', 'Page title', w, w, {
    severity: SEVERITY.info,
    fact: `"${title}"`,
    detail: { title },
  });
}

function checkDescription(meta) {
  const description = meta.get('description') ?? '';
  const w = WEIGHTS.description;
  if (!description.trim()) {
    return check('description', 'Meta description', w, 0, {
      severity: SEVERITY.medium,
      fact: 'No meta description was found.',
      fix: 'Add a one-sentence meta description summarising the page in its own words.',
    });
  }
  return check('description', 'Meta description', w, w, {
    severity: SEVERITY.info,
    fact: `${description.length} characters present.`,
    detail: { description },
  });
}

/**
 * Structured data is graded on validity first: a JSON-LD block that fails to
 * parse is worse than no block at all, because the author believes it works.
 */
function checkStructuredData(html) {
  const blocks = jsonLdBlocks(html);
  const w = WEIGHTS.structuredDataValid;
  const broken = blocks.filter((b) => !b.ok);

  if (blocks.length === 0) {
    return check('structured-data', 'Structured data (JSON-LD)', w, 0, {
      severity: SEVERITY.medium,
      fact: 'No JSON-LD blocks were found.',
      fix: 'Add schema.org JSON-LD describing what this page is (Product, Article, Organization, LocalBusiness). It is the least ambiguous way to state facts about a page.',
      detail: { blocks: 0, broken: 0 },
    });
  }
  if (broken.length > 0) {
    return check('structured-data', 'Structured data (JSON-LD)', w, w * 0.25, {
      severity: SEVERITY.high,
      fact: `${broken.length} of ${blocks.length} JSON-LD blocks failed to parse.`,
      fix: `Fix the malformed block — invalid JSON is silently discarded, so this markup is currently doing nothing. First parse error: ${broken[0].error}`,
      detail: { blocks: blocks.length, broken: broken.length, firstError: broken[0].error },
    });
  }
  return check('structured-data', 'Structured data (JSON-LD)', w, w, {
    severity: SEVERITY.info,
    fact: `${blocks.length} valid JSON-LD block${blocks.length === 1 ? '' : 's'} found.`,
    detail: { blocks: blocks.length, broken: 0 },
  });
}

/** Product pages get an extra, non-scored completeness report on their offer. */
function checkProductCompleteness(html) {
  const products = productNodes(html);
  if (products.length === 0) return null;

  const offer = extractOffer(html);
  const missing = [];
  const product = products[0];
  if (!product.name) missing.push('name');
  if (!product.image) missing.push('image');
  if (!product.description) missing.push('description');
  if (offer.price === null) missing.push('price');
  if (!offer.currency) missing.push('priceCurrency');
  if (offer.inStock === null) missing.push('availability');
  if (!product.sku && !product.gtin && !product.gtin13 && !product.mpn) missing.push('sku or gtin');

  if (missing.length === 0) {
    return check('product-completeness', 'Product markup completeness', 0, 0, {
      severity: SEVERITY.info,
      fact: `Product markup includes name, image, description, price, currency, availability and an identifier.`,
      detail: { missing: [], offer },
    });
  }
  return check('product-completeness', 'Product markup completeness', 0, 0, {
    severity: missing.length > 3 ? SEVERITY.high : SEVERITY.medium,
    fact: `Product markup is missing: ${missing.join(', ')}.`,
    fix: 'Fill in the missing offer fields. Incomplete Product markup is frequently ignored wholesale rather than partially used.',
    detail: { missing, offer },
  });
}

function checkHeadings(html) {
  const h1 = countTags(html, 'h1');
  const total = ['h1', 'h2', 'h3', 'h4', 'h5', 'h6'].reduce((n, t) => n + countTags(html, t), 0);
  const w = WEIGHTS.headings;
  if (h1 === 0) {
    return check('headings', 'Heading structure', w, total > 0 ? w * 0.4 : 0, {
      severity: SEVERITY.medium,
      fact: `No <h1> found (${total} headings total).`,
      fix: 'Give the page exactly one <h1> naming its subject.',
      detail: { h1, total },
    });
  }
  if (h1 > 1) {
    return check('headings', 'Heading structure', w, w * 0.6, {
      severity: SEVERITY.low,
      fact: `${h1} <h1> elements found.`,
      fix: 'Use a single <h1> so the page has one unambiguous subject.',
      detail: { h1, total },
    });
  }
  return check('headings', 'Heading structure', w, w, {
    severity: SEVERITY.info,
    fact: `One <h1> and ${total} headings in total.`,
    detail: { h1, total },
  });
}

function checkLandmarks(html) {
  const main = countTags(html, 'main');
  const article = countTags(html, 'article');
  const w = WEIGHTS.landmarks;
  if (main === 0 && article === 0) {
    return check('landmarks', 'Semantic landmarks', w, 0, {
      severity: SEVERITY.low,
      fact: 'No <main> or <article> element was found.',
      fix: 'Wrap the primary content in <main> (or <article>) so the substance is distinguishable from navigation and chrome.',
      detail: { main, article },
    });
  }
  return check('landmarks', 'Semantic landmarks', w, w, {
    severity: SEVERITY.info,
    fact: `Found ${main} <main> and ${article} <article> element(s).`,
    detail: { main, article },
  });
}

function checkLang(html) {
  const lang = htmlLang(html);
  const w = WEIGHTS.lang;
  if (!lang) {
    return check('lang', 'Declared language', w, 0, {
      severity: SEVERITY.low,
      fact: 'The <html> element has no lang attribute.',
      fix: 'Add lang="en" (or the correct language) to <html>.',
    });
  }
  return check('lang', 'Declared language', w, w, {
    severity: SEVERITY.info,
    fact: `Declared as "${lang}".`,
    detail: { lang },
  });
}

function checkCanonical(html, finalUrl) {
  const href = canonicalHref(html);
  const w = WEIGHTS.canonical;
  if (!href) {
    return check('canonical', 'Canonical URL', w, w * 0.5, {
      severity: SEVERITY.low,
      fact: 'No canonical link element was found.',
      fix: 'Add <link rel="canonical"> so duplicate URLs resolve to one address.',
    });
  }
  let resolved = href;
  try {
    resolved = new URL(href, finalUrl).toString();
  } catch {
    return check('canonical', 'Canonical URL', w, 0, {
      severity: SEVERITY.medium,
      fact: `Canonical link is not a valid URL: "${href}".`,
      fix: 'Use an absolute, valid canonical URL.',
      detail: { href },
    });
  }
  return check('canonical', 'Canonical URL', w, w, {
    severity: SEVERITY.info,
    fact: resolved,
    detail: { href: resolved },
  });
}

function checkOpenGraph(meta) {
  const present = ['og:title', 'og:description', 'og:image', 'og:type'].filter((k) =>
    (meta.get(k) ?? '').trim(),
  );
  const w = WEIGHTS.openGraph;
  const earned = (present.length / 4) * w;
  if (present.length === 4) {
    return check('open-graph', 'Open Graph metadata', w, w, {
      severity: SEVERITY.info,
      fact: 'og:title, og:description, og:image and og:type are all present.',
      detail: { present },
    });
  }
  return check('open-graph', 'Open Graph metadata', w, earned, {
    severity: present.length === 0 ? SEVERITY.medium : SEVERITY.low,
    fact: `${present.length} of 4 core Open Graph tags present${present.length ? ` (${present.join(', ')})` : ''}.`,
    fix: 'Add the missing og: tags. They are widely reused as a page summary beyond social previews.',
    detail: { present },
  });
}

function checkImageAlt(html) {
  const { total, withAlt } = imageAltCoverage(html);
  const w = WEIGHTS.imageAlt;
  if (total === 0) {
    return check('image-alt', 'Image alt text', w, w, {
      severity: SEVERITY.info,
      fact: 'No <img> elements on the page.',
      detail: { total, withAlt },
    });
  }
  const ratio = withAlt / total;
  return check('image-alt', 'Image alt text', w, w * ratio, {
    severity: ratio === 1 ? SEVERITY.info : ratio >= 0.75 ? SEVERITY.low : SEVERITY.medium,
    fact: `${withAlt} of ${total} images have an alt attribute.`,
    fix: ratio === 1 ? null : 'Add alt text to the remaining images (alt="" is correct for purely decorative ones).',
    detail: { total, withAlt },
  });
}

function checkLlmsTxt(llms) {
  const w = WEIGHTS.llmsTxt;
  if (!llms.found) {
    return check('llms-txt', '/llms.txt', w, 0, {
      severity: SEVERITY.low,
      fact: 'No /llms.txt file was served.',
      fix: 'Optional and not a standard: a plain-text /llms.txt summarising your site and linking key pages. Cheap to add, and some tools read it.',
    });
  }
  return check('llms-txt', '/llms.txt', w, w, {
    severity: SEVERITY.info,
    fact: `Present (${llms.body.length} bytes).`,
  });
}

/**
 * Runs every check against a fetched page.
 *
 * @param {object} page result of fetchPage()
 * @param {object} support { robots, llms } results of fetchSibling()
 */
export function auditPage(page, support = {}) {
  const html = page.body ?? '';
  const meta = metaTags(html);
  const robots = support.robots ?? { found: false, body: null };
  const llms = support.llms ?? { found: false, body: null };

  const scored = [
    checkContentWithoutJs(html),
    checkTitle(html),
    checkDescription(meta),
    checkStructuredData(html),
    checkHeadings(html),
    checkLandmarks(html),
    checkLang(html),
    checkCanonical(html, page.url),
    checkOpenGraph(meta),
    checkImageAlt(html),
    checkLlmsTxt(llms),
  ];

  const unscored = [checkProductCompleteness(html)].filter(Boolean);

  const totalWeight = scored.reduce((sum, c) => sum + c.weight, 0);
  const earned = scored.reduce((sum, c) => sum + c.earned, 0);
  const score = totalWeight === 0 ? 0 : Math.round((earned / totalWeight) * 100);

  const path = (() => {
    try {
      return new URL(page.url).pathname || '/';
    } catch {
      return '/';
    }
  })();

  return {
    url: page.url,
    status: page.status,
    elapsedMs: page.elapsedMs,
    redirects: page.redirects ?? [],
    truncated: Boolean(page.truncated),
    score,
    grade: score >= 85 ? 'A' : score >= 70 ? 'B' : score >= 55 ? 'C' : score >= 40 ? 'D' : 'F',
    checks: scored,
    notes: unscored,
    crawlers: robots.found ? aiCrawlerStatus(robots.body, path) : null,
    robotsFound: robots.found,
    scannedAt: Date.now(),
  };
}

/** Findings worth acting on, most severe first. */
export function prioritisedFindings(report) {
  const order = { high: 0, medium: 1, low: 2, info: 3 };
  return [...report.checks, ...report.notes]
    .filter((c) => c.fix && c.severity !== SEVERITY.info)
    .sort((a, b) => order[a.severity] - order[b.severity] || b.weight - a.weight);
}
