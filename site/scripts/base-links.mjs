// Markdown links in the pages are written root-relative (`/guides/baseline/`).
// This hast plugin for the Sätteri Markdown processor prefixes them with the
// site's base path, so the site also works when it is served from a sub-path.

export function baseLinks(base) {
	const prefix = base.replace(/\/$/, '');
	return {
		name: 'iglint-base-links',
		element: {
			filter: ['a'],
			visit(node, ctx) {
				const href = node.properties?.href;
				if (!prefix || typeof href !== 'string') return;
				if (!href.startsWith('/') || href.startsWith('//') || href.startsWith(`${prefix}/`)) return;
				ctx.setProperty(node, 'href', `${prefix}${href}`);
			},
		},
	};
}
