// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { satteri } from '@astrojs/markdown-satteri';

import { BASE, REPO_BRANCH, REPO_URL, SITE_URL } from './site.config.mjs';
import { baseLinks } from './scripts/base-links.mjs';
import { readRuleIndex, ruleSlug } from './scripts/rules-index.mjs';

/** One collapsed sidebar group per rule category, from docs/rules/README.md. */
const ruleGroups = readRuleIndex().map((category) => ({
	label: `${category.title} (${category.prefix})`,
	collapsed: true,
	items: category.rules.map((rule) => ({
		label: `${rule.code} ${rule.name}`,
		slug: ruleSlug(rule.code),
	})),
}));

export default defineConfig({
	site: SITE_URL,
	base: BASE,
	trailingSlash: 'always',
	markdown: {
		// No smart punctuation: it would turn `--` into dashes and quotes into
		// curly quotes outside code.
		processor: satteri({ hastPlugins: [baseLinks(BASE)], features: { smartPunctuation: false } }),
	},
	integrations: [
		starlight({
			title: 'iglint',
			description:
				'A fast, precise linter for Ignition 8.3 projects: Perspective views, Jython 2.7 scripts, expressions and gateway resources.',
			logo: { src: './src/assets/mustry-mark.svg', alt: 'Mustry Solutions' },
			favicon: '/favicon.svg',
			customCss: ['./src/styles/mustry.css'],
			social: [{ icon: 'github', label: 'GitHub', href: REPO_URL }],
			editLink: { baseUrl: `${REPO_URL}/edit/${REPO_BRANCH}/site/` },
			lastUpdated: false,
			sidebar: [
				{
					label: 'Start here',
					items: [
						{ label: 'Introduction', slug: 'introduction' },
						{ label: 'Installation', slug: 'installation' },
						{ label: 'Getting started', slug: 'getting-started' },
					],
				},
				{
					label: 'What iglint checks',
					items: [
						{ label: 'Overview', slug: 'checks' },
						{ label: 'Scripts', slug: 'checks/scripts' },
						{ label: 'Expressions and bindings', slug: 'checks/expressions-and-bindings' },
						{ label: 'References between resources', slug: 'checks/references' },
						{ label: 'Security, performance, accessibility', slug: 'checks/security-performance-accessibility' },
						{ label: 'What iglint does not catch', slug: 'what-iglint-does-not-catch' },
					],
				},
				{
					label: 'Guides',
					items: [
						{ label: 'Configuration', slug: 'guides/configuration' },
						{ label: 'Suppressing findings', slug: 'guides/suppressions' },
						{ label: 'Baseline', slug: 'guides/baseline' },
						{ label: 'CI integration', slug: 'guides/ci' },
						{ label: 'pre-commit', slug: 'guides/pre-commit' },
						{ label: 'Editors', slug: 'guides/editors' },
						{ label: 'Migrating from ign-lint', slug: 'guides/migrating-from-ign-lint' },
						{ label: 'Performance and caching', slug: 'guides/performance' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Command line', slug: 'reference/cli' },
						{ label: 'Configuration file', slug: 'reference/configuration' },
						{ label: 'Output formats', slug: 'reference/output-formats' },
						{ label: 'Rules overview', slug: 'reference/rules' },
						...ruleGroups,
						{ label: 'Differences from ign-lint', slug: 'reference/ign-lint-differences' },
					],
				},
				{
					label: 'Project',
					items: [
						{ label: 'How iglint is validated', slug: 'project/validation' },
						{ label: 'FAQ and troubleshooting', slug: 'project/faq' },
						{ label: 'Contributing', slug: 'project/contributing' },
						{ label: 'License', slug: 'project/license' },
						{ label: 'Changelog', slug: 'project/changelog' },
						{ label: 'Fix safety (design notes)', slug: 'internals/fix-safety' },
					],
				},
			],
		}),
	],
});
