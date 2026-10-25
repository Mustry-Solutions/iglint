// Settings shared by astro.config.mjs and scripts/sync-docs.mjs.

// Public address of the site (GitHub Pages with a custom domain). Set
// IGLINT_DOCS_URL to build for another address; a URL with a path (for
// example https://mustry-solutions.github.io/iglint/) also sets the base path
// that every page and link is served under.
export const SITE_URL = process.env.IGLINT_DOCS_URL ?? 'https://iglint.mustrysolutions.com/';

/** Base path derived from SITE_URL: `/` or `/iglint/`. */
export const BASE = new URL(SITE_URL).pathname.replace(/\/?$/, '/');

export const REPO_URL = 'https://github.com/Mustry-Solutions/iglint';

/** Branch whose files the "edit" and source links point to. */
export const REPO_BRANCH = 'main';
