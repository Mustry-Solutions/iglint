// Reads the rule index that `iglint generate-docs` writes to docs/rules/README.md.
// Used by astro.config.mjs (sidebar) and scripts/sync-docs.mjs (pages), so the
// generated rule reference stays the single source of truth.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const RULES_DIR = join(REPO_ROOT, 'docs', 'rules');

/** Strips one pair of backticks: "`invalid-json`" -> "invalid-json". */
const unquote = (cell) => cell.trim().replace(/^`(.*)`$/, '$1');

/**
 * Categories in the order of the index, each with its rules:
 * `{ title, prefix, anchor, rules: [{ code, name, summary, severity, enabled, fix }] }`.
 */
export function readRuleIndex() {
	const text = readFileSync(join(RULES_DIR, 'README.md'), 'utf8');
	const categories = [];
	let current = null;
	for (const line of text.split('\n')) {
		const heading = line.match(/^## (.+) \(`([A-Z0-9]+)`\)\s*$/);
		if (heading) {
			current = { title: heading[1], prefix: heading[2], rules: [] };
			categories.push(current);
			continue;
		}
		const row = line.match(/^\| \[([A-Z0-9]+)\]\([A-Z0-9]+\.md\) \|(.*)\|\s*$/);
		if (row && current) {
			const [name, summary, severity, enabled, fix] = row[2].split(' | ').map((c) => c.trim());
			current.rules.push({
				code: row[1],
				name: unquote(name),
				summary,
				severity,
				enabled: enabled === 'yes',
				fix: fix || 'no',
			});
		}
	}
	if (categories.length === 0 || categories.some((c) => c.rules.length === 0)) {
		throw new Error(`could not read the rule index in ${RULES_DIR}/README.md`);
	}
	return categories;
}

/** Slug of a rule page: `reference/rules/scr101`. */
export const ruleSlug = (code) => `reference/rules/${code.toLowerCase()}`;
