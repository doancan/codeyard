// The Codeyard site: the home page and the extension Market. It reads the registry's own public files on the same
// address (/index.json and /packages/<digest>.details.json), so it needs no API of its own.
//
// What a publisher wrote (names, descriptions, READMEs) is only ever put into the page as text nodes: nothing from the
// registry is parsed as HTML. READMEs go through the small Markdown reader below, which builds elements itself, keeps
// only http(s) and mailto links, and shows no images (as the app's Market does).
'use strict';

const REPO = 'https://github.com/doancan/codeyard';
const DOWNLOAD = REPO + '/releases/latest';
const view = document.getElementById('view');

// ---------- small helpers ----------

/** An element with attributes and children; strings become text nodes. */
function h(tag, attrs, ...children) {
	const el = document.createElement(tag);
	for (const [k, v] of Object.entries(attrs || {})) {
		if (v === false || v == null) continue;
		if (k === 'class') el.className = v;
		else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
		else el.setAttribute(k, v === true ? '' : String(v));
	}
	append(el, children);
	return el;
}
function append(el, children) {
	for (const c of children.flat(Infinity)) {
		if (c == null || c === false) continue;
		el.append(c instanceof Node ? c : document.createTextNode(String(c)));
	}
	return el;
}
/** Replace an element's children; arrays are flattened and null or false left out (unlike `replaceChildren`). */
const fill = (el, ...children) => append((el.replaceChildren(), el), children);
const link = (href, attrs, ...children) => h('a', { href, 'data-link': '', ...attrs }, ...children);

function statusKind(s) {
	if (s == null) return 'ok';
	return typeof s === 'string' ? s : Object.keys(s)[0] || 'ok';
}
function compareVersions(a, b) {
	const pa = String(a).split(/[-+]/)[0].split('.').map(Number);
	const pb = String(b).split(/[-+]/)[0].split('.').map(Number);
	for (let i = 0; i < 3; i++) if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) - (pb[i] || 0);
	const preA = String(a).includes('-'), preB = String(b).includes('-');
	return preA === preB ? 0 : preA ? -1 : 1;
}
const isPre = (v) => String(v).split('+')[0].includes('-');
const installable = (r) => ['ok', 'deprecated'].includes(statusKind(r.status));

/** The version a new user gets: the newest stable one that is not yanked or revoked, else the newest pre-release. */
function latest(entry) {
	const live = entry.versions.filter(installable).sort((a, b) => compareVersions(a.version, b.version));
	return live.filter((r) => !isPre(r.version)).pop() || live.pop() || null;
}
function formatCount(n) {
	if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 0 : 1) + 'M';
	if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e4 ? 0 : 1) + 'k';
	return String(n);
}
function formatDate(secs) {
	if (!secs) return '';
	return new Date(secs * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}
function formatSize(bytes) {
	if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + ' MB';
	if (bytes >= 1 << 10) return Math.round(bytes / (1 << 10)) + ' KB';
	return bytes + ' B';
}
function publisherLabel(p) {
	if (!p) return 'unknown';
	if (p.subject) return p.subject;
	if (p.issuer === 'https://token.actions.githubusercontent.com' && p.repository_id) return 'GitHub repository #' + p.repository_id;
	return p.issuer;
}

// ---------- data ----------

let indexLoad = null;
/** The registry's index, as a list of { id, ...entry, latest, downloads }; entries with nothing to install are left out. */
function loadIndex() {
	indexLoad ||= fetch('/index.json', { cache: 'no-cache' })
		.then((r) => {
			if (!r.ok) throw new Error('The registry answered ' + r.status + '.');
			return r.json();
		})
		.then((index) =>
			Object.entries(index.extensions || {}).map(([id, e]) => ({
				id,
				...e,
				categories: e.categories || [],
				latest: latest(e),
				downloads: e.versions.reduce((n, r) => n + (r.downloads || 0), 0),
			})),
		)
		.catch((e) => {
			indexLoad = null;
			throw e;
		});
	return indexLoad;
}

async function sha256Hex(buf) {
	if (!crypto.subtle) return null;
	const d = await crypto.subtle.digest('SHA-256', buf);
	return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, '0')).join('');
}
/** A release's README and manifest, checked against the digest the index lists for them. */
async function loadDetails(release) {
	if (!release || !release.details) return null;
	const r = await fetch('/packages/' + encodeURIComponent(release.digest) + '.details.json');
	if (!r.ok) return null;
	const buf = await r.arrayBuffer();
	const hex = await sha256Hex(buf);
	if (hex !== null && hex !== release.details) return null;
	return JSON.parse(new TextDecoder().decode(buf));
}

// ---------- Markdown (READMEs) ----------

function safeHref(url) {
	const u = url.trim();
	return /^(https?:|mailto:)/i.test(u) ? u : null;
}
/** Inline Markdown to nodes: code, links, images (as their text), bold, italics, line breaks. */
function inline(text) {
	const out = [];
	const re = /(`+)([\s\S]*?[^`])\1(?!`)|!\[([^\]]*)\]\(([^)\s]*)(?:\s+"[^"]*")?\)|\[([^\]]+)\]\(([^)\s]*)(?:\s+"[^"]*")?\)|<(https?:\/\/[^\s>]+)>|\*\*([\s\S]+?)\*\*|__([\s\S]+?)__|\*([^*\s][^*]*?)\*|\b_([^_\s][^_]*?)_\b|~~([\s\S]+?)~~|( {2,}|\\)\n/g;
	let last = 0, m;
	while ((m = re.exec(text))) {
		if (m.index > last) out.push(text.slice(last, m.index));
		if (m[1]) out.push(h('code', null, m[2].replace(/^ (.*) $/s, '$1')));
		else if (m[3] !== undefined && m[0].startsWith('!')) out.push(m[3] ? '[image: ' + m[3] + ']' : '');
		else if (m[5] !== undefined) {
			const href = safeHref(m[6]);
			out.push(href ? h('a', { href, rel: 'noopener nofollow ugc' }, inline(m[5])) : inline(m[5]));
		} else if (m[7]) out.push(h('a', { href: m[7], rel: 'noopener nofollow ugc' }, m[7]));
		else if (m[8] || m[9]) out.push(h('strong', null, inline(m[8] || m[9])));
		else if (m[10] || m[11]) out.push(h('em', null, inline(m[10] || m[11])));
		else if (m[12]) out.push(h('del', null, inline(m[12])));
		else if (m[13]) out.push(h('br'));
		last = re.lastIndex;
	}
	if (last < text.length) out.push(text.slice(last));
	return out;
}
function tableCells(line) {
	return line.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
}
/** Block Markdown to a fragment: headings, fenced and indented code, lists, quotes, tables, rules, paragraphs. */
function markdown(src) {
	const lines = src.replace(/\r\n?/g, '\n').split('\n');
	const frag = document.createDocumentFragment();
	let i = 0;
	const isBlank = (l) => /^\s*$/.test(l);
	const starts = (l) => /^(#{1,6}\s|```|~~~|>|\s*([-*+]|\d+[.)])\s|\s*(\*\s*){3,}$|\s*(-\s*){3,}$|\s*(_\s*){3,}$)/.test(l);
	while (i < lines.length) {
		const line = lines[i];
		if (isBlank(line)) { i++; continue; }
		let m;
		if ((m = line.match(/^\s*(```+|~~~+)\s*([\w+-]*)/))) {
			const fence = m[1], body = [];
			i++;
			while (i < lines.length && !lines[i].trim().startsWith(fence)) body.push(lines[i++]);
			i++;
			frag.append(h('pre', null, h('code', m[2] ? { 'data-lang': m[2] } : null, body.join('\n'))));
		} else if ((m = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/))) {
			frag.append(h('h' + Math.min(6, m[1].length + 1), null, inline(m[2])));
			i++;
		} else if (/^\s*((\*\s*){3,}|(-\s*){3,}|(_\s*){3,})$/.test(line)) {
			frag.append(h('hr'));
			i++;
		} else if (/^ {4}|^\t/.test(line)) {
			const body = [];
			while (i < lines.length && (/^ {4}|^\t/.test(lines[i]) || isBlank(lines[i]))) body.push(lines[i++].replace(/^ {4}|^\t/, ''));
			frag.append(h('pre', null, h('code', null, body.join('\n').replace(/\n+$/, ''))));
		} else if (/^\s*>/.test(line)) {
			const body = [];
			while (i < lines.length && /^\s*>/.test(lines[i])) body.push(lines[i++].replace(/^\s*>\s?/, ''));
			frag.append(h('blockquote', null, markdown(body.join('\n'))));
		} else if ((m = line.match(/^(\s*)([-*+]|\d+[.)])\s+/))) {
			const ordered = /\d/.test(m[2]);
			const list = h(ordered ? 'ol' : 'ul');
			const indent = m[1].length;
			while (i < lines.length) {
				const lm = lines[i].match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/);
				if (!lm || lm[1].length !== indent || /\d/.test(lm[2]) !== ordered) break;
				const item = [lm[3]];
				i++;
				while (i < lines.length && !isBlank(lines[i]) && (lines[i].match(/^\s*/)[0].length > indent || !starts(lines[i]))) {
					item.push(lines[i++].slice(Math.min(indent + 2, lines[i - 1].match(/^\s*/)[0].length)));
				}
				const task = item[0].match(/^\[([ xX])\]\s+(.*)$/);
				if (task) item[0] = task[2];
				const li = h('li', null, task ? h('input', { type: 'checkbox', disabled: true, checked: task[1] !== ' ' }) : null, task ? ' ' : null);
				li.append(item.length > 1 ? markdown(item.join('\n')) : append(document.createDocumentFragment(), inline(item[0])));
				list.append(li);
				if (i < lines.length && isBlank(lines[i]) && lines[i + 1] && /^(\s*)([-*+]|\d+[.)])\s+/.test(lines[i + 1])) i++;
			}
			frag.append(list);
		} else if (line.includes('|') && lines[i + 1] && /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/.test(lines[i + 1])) {
			const head = tableCells(line);
			i += 2;
			const rows = [];
			while (i < lines.length && lines[i].includes('|') && !isBlank(lines[i])) rows.push(tableCells(lines[i++]));
			frag.append(
				h('table', null,
					h('thead', null, h('tr', null, head.map((c) => h('th', null, inline(c))))),
					h('tbody', null, rows.map((r) => h('tr', null, head.map((_, j) => h('td', null, inline(r[j] || '')))))),
				),
			);
		} else if (/^\s*<\/?[a-zA-Z]/.test(line)) {
			// Raw HTML is not rendered; skip the block (a README's centred logo, badges and the like).
			while (i < lines.length && !isBlank(lines[i])) i++;
		} else {
			const body = [];
			while (i < lines.length && !isBlank(lines[i]) && !(body.length && starts(lines[i]))) body.push(lines[i++]);
			frag.append(h('p', null, inline(body.join('\n'))));
		}
	}
	return frag;
}

// ---------- pages ----------

function setTitle(t) {
	document.title = t ? t + ' · Codeyard' : 'Codeyard';
}
function markNav(path) {
	for (const a of document.querySelectorAll('.nav a[data-link]')) {
		a.toggleAttribute('aria-current', path.startsWith(a.getAttribute('href')));
		if (a.hasAttribute('aria-current')) a.setAttribute('aria-current', 'page');
	}
}

function badges(e) {
	const r = e.latest;
	const kinds = new Set(e.versions.map((v) => statusKind(v.status)));
	return [
		r && statusKind(r.status) === 'deprecated' ? h('span', { class: 'badge deprecated' }, 'Deprecated') : null,
		!r && kinds.has('revoked') ? h('span', { class: 'badge revoked' }, 'Revoked') : null,
		!r && !kinds.has('revoked') ? h('span', { class: 'badge yanked' }, 'Withdrawn') : null,
		r && r.code ? h('span', { class: 'badge code', title: 'Runs its own programs; updates wait 48 hours.' }, 'Runs code') : null,
		r && isPre(r.version) ? h('span', { class: 'badge pre' }, 'Pre-release') : null,
	];
}

function card(e) {
	return link('/extensions/' + encodeURIComponent(e.id), { class: 'card' },
		h('h3', null, e.name || e.id),
		h('span', { class: 'id' }, e.id),
		h('p', null, e.description || 'No description.'),
		h('div', { class: 'meta' },
			e.latest ? h('span', null, 'v' + e.latest.version) : null,
			h('span', null, formatCount(e.downloads) + ' downloads'),
			badges(e),
		),
	);
}

const FEATURES = [
	['Local and SSH are the same', 'Open a server folder like a local one. Files, search, terminals, git and language servers run on the server through a tiny agent.'],
	['The editor you expect', 'Multiple cursors, split views, snippets, more than 20 languages, LSP, debugging, tests, tasks and a terminal with shell integration.'],
	['Search by structure', 'Find and rewrite code by its syntax tree, with $X placeholders, over local and SSH folders.'],
	['Git inside', 'Sync in one click, branch graph, timeline, blame, a three-way merge editor and GitHub pull requests.'],
	['Everyday tools built in', 'Schema validation, spell check, .http requests, log follow, Markdown preview, hex view and data conversion.'],
	['Native, no telemetry', 'Written in Rust on GPUI. No Electron, no web view, no account. Requests go out only when you start them.'],
];

async function homePage() {
	setTitle('');
	const popular = h('div', { class: 'grid' }, [1, 2, 3].map(() => h('div', { class: 'skeleton' })));
	const market = h('section', { class: 'section' },
		h('div', { class: 'section-head' }, h('h2', null, 'Popular extensions'), link('/extensions', null, 'Browse all →')),
		popular,
	);
	fill(view, 
		h('section', { class: 'hero' },
			h('img', { class: 'logo', src: '/codeyard.svg', alt: 'Codeyard logo', width: 96, height: 96 }),
			h('h1', null, 'Codeyard'),
			h('p', { class: 'lead' }, 'A small native code editor for local folders and remote servers over SSH. Written in Rust. No Electron, no telemetry.'),
			h('div', { class: 'actions' },
				h('a', { class: 'btn primary', href: DOWNLOAD, rel: 'noopener' }, 'Download'),
				link('/extensions', { class: 'btn' }, 'Browse extensions'),
			),
			h('p', { class: 'platforms' }, 'macOS (Apple silicon) and Windows · Beta'),
			h('img', { class: 'shot', src: '/hero.png', alt: 'Codeyard editing a Rust file with the explorer, outline and timeline open', width: 1800, height: 1100, loading: 'lazy' }),
		),
		h('section', { class: 'section' }, h('div', { class: 'features' }, FEATURES.map(([t, d]) => h('div', { class: 'feature' }, h('h3', null, t), h('p', null, d))))),
		market,
	);
	try {
		const list = (await loadIndex()).filter((e) => e.latest).sort((a, b) => b.downloads - a.downloads).slice(0, 6);
		if (!list.length) market.remove();
		else fill(popular, ...list.map(card));
	} catch {
		market.remove();
	}
}

async function listPage(params) {
	setTitle('Extensions');
	const state = { q: params.get('q') || '', c: params.get('c') || '', sort: params.get('sort') || 'popular' };
	const search = h('input', { type: 'search', placeholder: 'Search extensions', 'aria-label': 'Search extensions', value: state.q });
	const sort = h('select', { 'aria-label': 'Sort' },
		h('option', { value: 'popular' }, 'Most downloaded'),
		h('option', { value: 'recent' }, 'Recently updated'),
		h('option', { value: 'name' }, 'Name'),
	);
	sort.value = state.sort;
	const chips = h('div', { class: 'chips', role: 'group', 'aria-label': 'Categories' });
	const count = h('p', { class: 'count', 'aria-live': 'polite' });
	const grid = h('div', { class: 'grid' }, [1, 2, 3, 4, 5, 6].map(() => h('div', { class: 'skeleton' })));
	fill(view, 
		h('div', { class: 'section-head' }, h('h1', null, 'Extensions'),
			h('a', { href: REPO + '/blob/main/docs/registry.md', rel: 'noopener' }, 'Publish yours')),
		h('div', { class: 'toolbar' }, search, sort),
		chips, count, grid,
	);

	let all;
	try {
		all = (await loadIndex()).filter((e) => e.latest);
	} catch (err) {
		fill(grid, h('div', { class: 'empty' }, 'The extension list could not be loaded. ', String(err.message || err)));
		return;
	}

	const categories = [...new Set(all.flatMap((e) => e.categories))].sort((a, b) => a.localeCompare(b));
	function renderChips() {
		fill(chips, 
			...['', ...categories].map((c) =>
				h('button', { class: 'chip', type: 'button', 'aria-pressed': String(state.c === c), onclick: () => { state.c = c; update(); } }, c || 'All'),
			),
		);
	}
	function update() {
		const url = new URL(location.href);
		for (const k of ['q', 'c', 'sort']) {
			if (state[k] && !(k === 'sort' && state[k] === 'popular')) url.searchParams.set(k, state[k]);
			else url.searchParams.delete(k);
		}
		history.replaceState(null, '', url);
		renderChips();
		const words = state.q.toLowerCase().split(/\s+/).filter(Boolean);
		const shown = all
			.filter((e) => !state.c || e.categories.includes(state.c))
			.filter((e) => {
				const hay = [e.id, e.name, e.description, ...e.categories].join(' ').toLowerCase();
				return words.every((w) => hay.includes(w));
			})
			.sort((a, b) =>
				state.sort === 'name' ? (a.name || a.id).localeCompare(b.name || b.id)
				: state.sort === 'recent' ? b.latest.published - a.latest.published
				: b.downloads - a.downloads,
			);
		count.textContent = shown.length === 1 ? '1 extension' : shown.length + ' extensions';
		fill(grid, ...(shown.length ? shown.map(card) : [h('div', { class: 'empty' }, all.length ? 'No extension matches.' : 'No extensions are published yet.')]));
	}
	search.addEventListener('input', () => { state.q = search.value; update(); });
	sort.addEventListener('change', () => { state.sort = sort.value; update(); });
	update();
}

function copyButton(text) {
	const b = h('button', { class: 'btn small', type: 'button' }, 'Copy');
	b.addEventListener('click', async () => {
		try {
			await navigator.clipboard.writeText(text);
			b.textContent = 'Copied';
		} catch {
			b.textContent = 'Select and copy';
		}
		setTimeout(() => (b.textContent = 'Copy'), 1600);
	});
	return b;
}

async function extensionPage(id) {
	setTitle(id);
	fill(view, h('div', { class: 'skeleton' }));
	let e;
	try {
		e = (await loadIndex()).find((x) => x.id === id);
	} catch (err) {
		fill(view, h('div', { class: 'empty' }, 'The extension list could not be loaded. ', String(err.message || err)));
		return;
	}
	if (!e) {
		fill(view, h('div', { class: 'empty' }, h('h1', null, 'Not found'), h('p', null, 'No extension is published as ', h('code', null, id), '.'), link('/extensions', null, 'All extensions')));
		return;
	}
	setTitle(e.name || e.id);
	const r = e.latest;
	const notices = [];
	const shownStatus = r ? r.status : e.versions.at(-1)?.status;
	if (statusKind(shownStatus) === 'deprecated') {
		const repl = shownStatus.deprecated && shownStatus.deprecated.replacement;
		notices.push(h('div', { class: 'notice warn' }, 'This extension is deprecated.', repl ? [' Use ', link('/extensions/' + encodeURIComponent(repl), null, repl), ' instead.'] : null));
	}
	if (!r) {
		const revoked = e.versions.map((v) => v.status).find((s) => statusKind(s) === 'revoked');
		notices.push(h('div', { class: 'notice danger' }, revoked ? 'Every version was revoked: ' + revoked.revoked.reason : 'Every version was withdrawn by its publisher.'));
	}

	const command = 'codeyard --install-extension ' + e.id;
	const readme = h('div', { class: 'readme' }, h('p', { class: 'note' }, r ? 'Loading…' : ''));
	const side = h('aside', null,
		r ? h('div', { class: 'panel' },
			h('h2', null, 'Install'),
			h('p', { class: 'note', style: null }, 'In Codeyard, open Extensions and search for ', h('code', null, e.id), ', or run:'),
			h('div', { class: 'install' }, h('code', null, command), copyButton(command)),
			r.code ? h('p', { class: 'note' }, 'This extension runs its own programs. Codeyard shows what it may do before it runs, and its updates wait 48 hours.') : null,
		) : null,
		h('div', { class: 'panel' },
			h('h2', null, 'About'),
			h('dl', null,
				r ? [h('dt', null, 'Version'), h('dd', null, r.version)] : null,
				r ? [h('dt', null, 'Updated'), h('dd', null, formatDate(r.published))] : null,
				r ? [h('dt', null, 'Size'), h('dd', null, formatSize(r.size))] : null,
				[h('dt', null, 'Downloads'), h('dd', null, e.downloads.toLocaleString())],
				[h('dt', null, 'Publisher'), h('dd', null, publisherLabel(e.publisher))],
				e.categories.length ? [h('dt', null, 'Categories'), h('dd', null, e.categories.map((c, j) => [j ? ', ' : '', link('/extensions?c=' + encodeURIComponent(c), null, c)]))] : null,
				r && Object.keys(r.targets || {}).length ? [h('dt', null, 'Platforms'), h('dd', null, Object.keys(r.targets).join(', '))] : null,
			),
		),
	);
	const versions = [...e.versions].sort((a, b) => compareVersions(b.version, a.version));
	fill(view, 
		h('nav', { class: 'crumbs', 'aria-label': 'Breadcrumb' }, link('/extensions', null, 'Extensions'), ' / ', e.id),
		h('div', { class: 'ext-head' },
			h('div', null,
				h('h1', null, e.name || e.id),
				h('div', { class: 'id' }, e.id),
				e.description ? h('p', { class: 'desc' }, e.description) : null,
				h('div', { class: 'meta', style: null }, badges(e)),
			),
		),
		notices,
		h('div', { class: 'layout' },
			h('div', null,
				h('div', { class: 'panel' }, readme),
				h('div', { class: 'panel' },
					h('h2', null, 'Versions'),
					h('div', { class: 'table-scroll' },
						h('table', { class: 'versions' },
							h('thead', null, h('tr', null, h('th', null, 'Version'), h('th', null, 'Published'), h('th', null, 'Downloads'), h('th', null, 'Status'))),
							h('tbody', null, versions.map((v) => {
								const k = statusKind(v.status);
								return h('tr', null,
									h('td', null, v.version, isPre(v.version) ? [' ', h('span', { class: 'badge pre' }, 'pre')] : null),
									h('td', null, formatDate(v.published)),
									h('td', null, (v.downloads || 0).toLocaleString()),
									h('td', null, k === 'ok' ? '' : h('span', { class: 'badge ' + k }, k === 'yanked' ? 'withdrawn' : k)),
								);
							})),
						),
					),
				),
			),
			side,
		),
	);

	if (!r) return;
	let details = null;
	try {
		details = await loadDetails(r);
	} catch {
		details = null;
	}
	if (!document.body.contains(readme)) return;
	if (!details) {
		fill(readme, h('p', { class: 'note' }, 'This version has no README to show.'));
		return;
	}
	fill(readme, 
		details.readme ? markdown(details.readme) : h('p', { class: 'note' }, 'This version has no README.'),
		details.readme_cut ? h('p', { class: 'note' }, 'The README is longer; the rest is in the extension.') : null,
		details.manifest ? h('details', { class: 'manifest' }, h('summary', null, 'Manifest (codeyard-extension.toml)'), h('pre', null, details.manifest)) : null,
	);
}

// ---------- routing ----------

function route() {
	const path = decodeURIComponent(location.pathname.replace(/\/+$/, '')) || '/';
	markNav(path);
	window.scrollTo(0, 0);
	if (path === '/') return homePage();
	if (path === '/extensions') return listPage(new URLSearchParams(location.search));
	const m = path.match(/^\/extensions\/([^/]+)$/);
	if (m) return extensionPage(m[1]);
	setTitle('Not found');
	fill(view, h('div', { class: 'empty' }, h('h1', null, 'Not found'), h('p', null, 'There is no page here. '), link('/', null, 'Home')));
}

document.addEventListener('click', (ev) => {
	const a = ev.target.closest('a[data-link]');
	if (!a || ev.defaultPrevented || ev.button !== 0 || ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
	const url = new URL(a.href, location.href);
	if (url.origin !== location.origin) return;
	ev.preventDefault();
	if (url.href !== location.href) history.pushState(null, '', url);
	route();
	view.focus({ preventScroll: true });
});
window.addEventListener('popstate', route);
route();
