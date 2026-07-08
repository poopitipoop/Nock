# Pearl (pearlified.com)

A static marketing site for the Pearl (PRL) protocol — a Proof-of-Useful-Work L1 where
mining is matrix multiplication. Plain HTML/CSS/JS, no build step, no dependencies.

## Files

- `index.html` — the whole site (single page, anchor-linked sections)
- `styles.css` — dark-velvet / iridescent "nacre" design system
- `script.js` — mobile nav toggle + scroll-reveal animation
- `assets/favicon.svg` — pearl-mark favicon
- `assets/pearl-whitepaper.pdf` — the whitepaper, served directly from the site

## Deploy to Cloudflare Pages

### Option A — Direct upload (fastest, no git needed)

1. Go to the Cloudflare dashboard → **Workers & Pages** → **Create application** → **Pages** → **Upload assets**.
2. Give the project a name (e.g. `pearlified`).
3. Drag the entire `pearlified-site` folder (or a zip of its contents) into the upload box.
4. Deploy. Cloudflare gives you a `*.pages.dev` URL immediately.
5. In the project's **Custom domains** tab, add `pearlified.com` (and `www.pearlified.com` if
   desired). If the domain's DNS already lives on this Cloudflare account, the records are
   created for you automatically.

Whenever you want to update the site, repeat step 3–4 with the new files (or use "Manage
deployments" to upload a new version).

### Option B — Connect a git repository (auto-deploys on push)

1. Push this folder to a GitHub/GitLab repo (it can be a subfolder of a larger repo).
2. In Cloudflare: **Workers & Pages** → **Create application** → **Pages** → **Connect to Git**.
3. Pick the repo and branch.
4. Build settings:
   - **Build command**: (leave empty — there is no build step)
   - **Build output directory**: `pearlified-site` (or `/` if this folder is the repo root)
5. Deploy, then add `pearlified.com` under **Custom domains** as in Option A.

## Local preview

No build tooling required — just serve the folder:

```bash
cd pearlified-site
python3 -m http.server 8080
# open http://localhost:8080
```

## Updating content

Everything is hand-written HTML/CSS — no templating. All copy lives in `index.html`, all
visual styling in `styles.css`. The pearl-formation metaphor in the "Pearlification Process"
section maps directly onto Section 3 of the whitepaper (noise generation → tiled MatMul →
hash-based PoW → zk-SNARK opening), so keep the two in sync if the protocol design changes.
