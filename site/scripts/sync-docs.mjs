// Copies the documentation that lives next to the code into the site, before
// every `npm run dev` and `npm run build`:
//
// * docs/rules/*.md (written by `iglint generate-docs` from the rule metadata)
//   -> src/content/docs/reference/rules/, plus the rules overview page;
// * docs/compat/ign-lint-differences.md -> reference/ign-lint-differences.md;
// * docs/internal/fix-safety.md -> internals/fix-safety.md;
// * CHANGELOG.md -> project/changelog.md.
//
// The copies are gitignored: edit the sources, never the copies. Links
// between those files become site links; links to other repository files
// point to GitHub.

import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, posix } from 'node:path';
import { fileURLToPath } from 'node:url';

import { REPO_BRANCH, REPO_URL } from '../site.config.mjs';
import { REPO_ROOT, RULES_DIR, readRuleIndex, ruleSlug } from './rules-index.mjs';

const SITE_DIR = join(dirname(fileURLToPath(import.meta.url)), '..');
const CONTENT = join(SITE_DIR, 'src', 'content', 'docs');

/** Repository files that are pages of the site, by repository path. */
const PAGES = {
	'README.md': 'introduction',
	'CHANGELOG.md': 'project/changelog',
	'CONTRIBUTING.md': 'project/contributing',
	'docs/rules/README.md': 'reference/rules',
	'docs/compat/ign-lint-differences.md': 'reference/ign-lint-differences',
	'docs/internal/fix-safety.md': 'internals/fix-safety',
};

/** Sections of README.md that have a page of their own on the site. */
const README_ANCHORS = {
	configuration: 'guides/configuration',
	'migrating-from-ign-lint': 'guides/migrating-from-ign-lint',
	'suppressing-findings': 'guides/suppressions',
	baseline: 'guides/baseline',
	'output-formats-and-ci': 'guides/ci',
	'pre-commit': 'guides/pre-commit',
	editors: 'guides/editors',
	installation: 'installation',
	rules: 'reference/rules',
};

/** The site link (root-relative, before the base path) for a repository path. */
function siteLink(repoPath, anchor) {
	const rule = repoPath.match(/^docs\/rules\/([A-Z0-9]+)\.md$/);
	if (rule && rule[1] !== 'README') return `/${ruleSlug(rule[1])}/${anchor ? `#${anchor}` : ''}`;
	if (repoPath === 'README.md' && anchor && README_ANCHORS[anchor]) {
		return `/${README_ANCHORS[anchor]}/`;
	}
	const page = PAGES[repoPath];
	if (page && repoPath !== 'README.md') return `/${page}/${anchor ? `#${anchor}` : ''}`;
	return `${REPO_URL}/blob/${REPO_BRANCH}/${repoPath}${anchor ? `#${anchor}` : ''}`;
}

/** Rewrites the Markdown links of `text`, read from `repoPath`, outside code. */
function rewriteLinks(text, repoPath) {
	const from = posix.dirname(repoPath);
	const rewrite = (line) =>
		line.replace(/\]\(([^)\s]+)\)/g, (match, target) => {
			if (/^[a-z]+:/i.test(target) || target.startsWith('#') || target.startsWith('/')) return match;
			const [path, anchor] = target.split('#');
			const resolved = posix.normalize(posix.join(from, path));
			return `](${siteLink(resolved, anchor)})`;
		});
	let fence = null;
	return text
		.split('\n')
		.map((line) => {
			const open = line.match(/^\s*(```+|~~~+)/);
			if (open) {
				if (fence === null) fence = open[1];
				else if (line.trim().startsWith(fence)) fence = null;
				return line;
			}
			return fence === null ? rewrite(line) : line;
		})
		.join('\n');
}

/** Drops the generated-file comment and the first `# Title` line. */
function stripHeader(text) {
	return text
		.replace(/^<!--[^]*?-->\s*/, '')
		.replace(/^# .*\n+/, '');
}

const yaml = (value) => JSON.stringify(value);

function frontmatter(fields) {
	const lines = Object.entries(fields).map(([key, value]) =>
		typeof value === 'object' && value !== null
			? `${key}:\n${Object.entries(value).map(([k, v]) => `  ${k}: ${yaml(v)}`).join('\n')}`
			: `${key}: ${yaml(value)}`,
	);
	return `---\n${lines.join('\n')}\n---\n\n`;
}

function write(slug, content) {
	const file = join(CONTENT, `${slug}.md`);
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, content);
}

const sourceNote = (repoPath, how) =>
	`\n\n---\n\n*${how} Source: [\`${repoPath}\`](${REPO_URL}/blob/${REPO_BRANCH}/${repoPath}).*\n`;

// Rule pages and the overview.
const categories = readRuleIndex();
rmSync(join(CONTENT, 'reference', 'rules'), { recursive: true, force: true });
let count = 0;
for (const category of categories) {
	for (const rule of category.rules) {
		const repoPath = `docs/rules/${rule.code}.md`;
		const body = stripHeader(readFileSync(join(REPO_ROOT, repoPath), 'utf8'));
		write(
			ruleSlug(rule.code),
			frontmatter({ title: `${rule.code}: ${rule.name}`, description: rule.summary, editUrl: false }) +
				rewriteLinks(body, repoPath).trimEnd() +
				sourceNote(
					repoPath,
					`Generated from the rule metadata by \`iglint generate-docs\`; \`iglint rule ${rule.code}\` prints the same page in the terminal.`,
				),
		);
		count++;
	}
}
const pageFiles = readdirSync(RULES_DIR).filter((f) => /^[A-Z0-9]+\.md$/.test(f) && f !== 'README.md');
if (pageFiles.length !== count) {
	throw new Error(`docs/rules has ${pageFiles.length} rule pages but its index lists ${count} rules`);
}

const fixLabel = { always: 'always', sometimes: 'sometimes', no: 'no' };
const onByDefault = categories.flatMap((c) => c.rules).filter((r) => r.enabled).length;
const withFix = categories.flatMap((c) => c.rules).filter((r) => r.fix !== 'no').length;
let overview = frontmatter({
	title: 'Rules',
	description: `All ${count} iglint rules by category, with severity, default and fix availability.`,
	editUrl: false,
});
overview += `iglint has **${count} rules** in ${categories.length} categories. **${onByDefault}** run by default; enable the others with \`select\` or \`extend-select\` ([configuration](/guides/configuration/)). **${withFix}** can fix findings: *always* or *sometimes* means \`iglint check --fix\` can repair a finding of the rule, and fixes that are not proven safe also need \`--unsafe-fixes\` (see [safe and unsafe fixes](/what-iglint-does-not-catch/#safe-and-unsafe-fixes)).\n\n`;
overview += `Selectors accept the code (\`SCR101\`), the name (\`undefined-name\`), a code prefix (\`SCR1\`), a category (\`SCR\`) or \`ALL\`.\n\n`;
for (const category of categories) {
	overview += `## ${category.title} (\`${category.prefix}\`)\n\n`;
	overview += '| Code | Name | Severity | Default | Fix | Summary |\n|---|---|---|---|---|---|\n';
	for (const r of category.rules) {
		overview += `| [${r.code}](/${ruleSlug(r.code)}/) | \`${r.name}\` | ${r.severity} | ${r.enabled ? 'on' : 'off'} | ${fixLabel[r.fix] ?? r.fix} | ${r.summary} |\n`;
	}
	overview += '\n';
}
overview += sourceNote('docs/rules/README.md', 'Generated from the rule metadata by `iglint generate-docs`.');
write('reference/rules/index', overview);

// Other documents.
const documents = [
	{
		repoPath: 'docs/compat/ign-lint-differences.md',
		title: 'Differences from ign-lint',
		description: 'Every place where iglint intentionally reports something different from ign-lint 0.6.4, and why.',
	},
	{
		repoPath: 'docs/internal/fix-safety.md',
		title: 'Fix safety (design notes)',
		description: 'The internal argument for when a fix is safe: threat model, closed world, vetted code and the post-fix safety net.',
	},
	{
		repoPath: 'CHANGELOG.md',
		title: 'Changelog',
		description: 'Notable changes to iglint.',
	},
];
for (const doc of documents) {
	const body = stripHeader(readFileSync(join(REPO_ROOT, doc.repoPath), 'utf8'));
	write(
		PAGES[doc.repoPath],
		frontmatter({ title: doc.title, description: doc.description, editUrl: `${REPO_URL}/edit/${REPO_BRANCH}/${doc.repoPath}` }) +
			rewriteLinks(body, doc.repoPath).trimEnd() +
			sourceNote(doc.repoPath, 'Copied from the repository when the site is built.'),
	);
}

console.log(`sync-docs: ${count} rule pages, the rules overview and ${documents.length} documents`);
