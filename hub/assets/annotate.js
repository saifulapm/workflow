// The comment layer the hub adds to a designed plan page. A chat button turns
// comment mode on; a tap then drops a pin and opens a box, and each comment is
// posted to the hub page around the frame on its own. Decision cards get a
// "Send change" button. The page runs in a sandboxed frame with no storage, so
// everything it keeps lives in the parent, reached through postMessage.
(function () {
	'use strict';

	// <root>/<n>@<x>,<y>: n indexes root.querySelectorAll('*'), x and y are the
	// tap as whole percents of that element's box.
	var ELEMENT_ANCHOR = /^(claim-[A-Za-z0-9_-]+|scope|shared|page)\/(\d+)@(\d+),(\d+)$/;
	var DECISION_ANCHOR = /^decision-([^@\s]+)@(.+)$/;

	function formatAnchor(a) {
		return a.root + '/' + a.n + '@' + Math.round(a.x) + ',' + Math.round(a.y);
	}

	function parseAnchor(s) {
		if (typeof s !== 'string') return null;
		var m = ELEMENT_ANCHOR.exec(s);
		if (m) return { root: m[1], n: +m[2], x: +m[3], y: +m[4] };
		m = DECISION_ANCHOR.exec(s);
		if (m) return { decision: m[1], value: m[2] };
		return null;
	}

	function clampQuote(s) {
		return String(s || '').replace(/\s+/g, ' ').trim().slice(0, 120);
	}

	// Decisions the reader never touched and never sent a change for.
	function unopenedCount(names, seen, pins) {
		var sent = {};
		pins.forEach(function (p) {
			var a = parseAnchor(p.anchor);
			if (a && a.decision) sent[a.decision] = true;
		});
		return names.filter(function (n) { return !seen[n] && !sent[n]; }).length;
	}

	if (typeof module === 'object' && module.exports) {
		module.exports = { formatAnchor: formatAnchor, parseAnchor: parseAnchor, clampQuote: clampQuote, unopenedCount: unopenedCount };
	}
	if (typeof document === 'undefined') return;

	// Page tokens first, with fallbacks for a page that has none.
	var TOKENS = ':host{all:initial;' +
		'--h-surface:var(--surface,#fff);--h-line:var(--line,#d9d9d4);--h-ink:var(--ink,#17181c);--h-mut:var(--mut,#62646c);' +
		'--h-accent:var(--accent,#2b59c3);--h-accent-soft:var(--accent-soft,#e8eefb);--h-wait:var(--wait,#9a5b00);' +
		'font:15px/1.45 var(--font,ui-sans-serif,system-ui,sans-serif);color:var(--h-ink)}' +
		'@media (prefers-color-scheme:dark){:host{' +
		'--h-surface:var(--surface,#171a21);--h-line:var(--line,#323744);--h-ink:var(--ink,#e7e9ee);--h-mut:var(--mut,#a3a8b3);' +
		'--h-accent:var(--accent,#7ea2f2);--h-accent-soft:var(--accent-soft,#1c2742);--h-wait:var(--wait,#e7ad52)}}' +
		':host([hidden]){display:none}*{box-sizing:border-box}[hidden]{display:none!important}' +
		'button{font:inherit;color:inherit;cursor:pointer}';

	var LAYER_CSS = TOKENS +
		'.chat{position:fixed;right:calc(16px + env(safe-area-inset-right));bottom:calc(16px + env(safe-area-inset-bottom));' +
		'width:52px;height:52px;border-radius:50%;border:0;background:var(--h-accent);color:var(--h-surface);display:grid;place-items:center;' +
		'box-shadow:0 4px 14px rgba(0,0,0,.25)}' +
		'.chat[aria-pressed="true"]{outline:3px solid var(--h-accent-soft);outline-offset:2px}' +
		'.bar{position:fixed;left:0;right:0;top:0;padding:calc(8px + env(safe-area-inset-top)) 16px 8px;display:flex;align-items:center;' +
		'justify-content:center;gap:14px;background:var(--h-accent-soft);color:var(--h-accent);font-weight:700;font-size:14px;' +
		'border-bottom:1px solid var(--h-line)}' +
		'.bar button{border:1px solid var(--h-accent);background:var(--h-surface);color:var(--h-accent);border-radius:8px;padding:6px 14px;min-height:36px}' +
		'.pin{position:absolute;width:28px;height:28px;margin:-14px 0 0 -14px;border-radius:50% 50% 50% 4px;border:2px solid var(--h-surface);' +
		'background:var(--h-accent);color:var(--h-surface);font-weight:700;font-size:13px;padding:0;display:grid;place-items:center;' +
		'box-shadow:0 2px 6px rgba(0,0,0,.3)}' +
		'.pin.draft{background:var(--h-wait)}' +
		'.box{position:absolute;width:300px;background:var(--h-surface);color:var(--h-ink);border:1px solid var(--h-line);border-radius:12px;' +
		'padding:10px 12px 12px;display:grid;gap:8px;box-shadow:0 8px 28px rgba(0,0,0,.22)}' +
		'@media (max-width:599px){.box{position:fixed;left:0!important;right:0;bottom:0;top:auto!important;width:auto;border-radius:16px 16px 0 0;' +
		'padding-bottom:calc(14px + env(safe-area-inset-bottom))}}' +
		'.head{display:flex;align-items:center;gap:8px;font-size:12px;color:var(--h-mut)}' +
		'.head .title{flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}' +
		'.x{border:0;background:none;color:var(--h-mut);font-size:20px;line-height:1;width:32px;height:32px}' +
		'textarea{font:inherit;font-size:16px;color:var(--h-ink);background:transparent;border:1px solid var(--h-line);border-radius:8px;' +
		'padding:8px;min-height:72px;resize:vertical;width:100%}' +
		'textarea:focus{outline:2px solid var(--h-accent);outline-offset:-1px}' +
		'.row{display:flex;align-items:center;gap:10px;font-size:13px}.row label{flex:1;display:flex;align-items:center;gap:6px}' +
		'.row input{accent-color:var(--h-accent);width:16px;height:16px;margin:0}' +
		'.send{width:36px;height:36px;border-radius:50%;border:0;background:var(--h-accent);color:var(--h-surface);font-size:18px;font-weight:700}' +
		'.send:disabled{opacity:.5;cursor:default}' +
		'.cancel{border:0;background:none;color:var(--h-mut);font-size:13px;padding:6px}' +
		'.status{font-size:13px;color:var(--h-mut)}.status.err{color:var(--h-wait)}' +
		'.msg{display:grid;gap:2px}.who{font-size:12px;font-weight:700;color:var(--h-mut)}.who.claude{color:var(--h-accent)}' +
		'.text{white-space:pre-wrap;overflow-wrap:anywhere}.note{font-size:12px;color:var(--h-mut)}';

	var CHANGE_CSS = TOKENS +
		':host{display:flex}.change{display:flex;flex-wrap:wrap;align-items:center;gap:10px;font-size:13px;color:var(--h-mut)}' +
		'button{border:0;border-radius:8px;background:var(--h-accent);color:var(--h-surface);font-weight:700;padding:8px 14px;min-height:40px}' +
		'button:disabled{opacity:.6;cursor:default}.err{color:var(--h-wait)}';

	var framed = window.parent !== window;
	var pageRoot = document.querySelector('main[data-plan]') || document.body;
	var pins = readPins();
	var fieldsets = Array.prototype.slice.call(document.querySelectorAll('fieldset[data-decision]'));
	var names = fieldsets.map(function (f) { return f.dataset.decision; })
		.filter(function (n, i, all) { return all.indexOf(n) === i; });
	var seen = {};
	var started = false;
	var pending = {}; // anchor -> what to do when plan-sent answers for it
	var mode = false;
	var draft = null; // the one unsent comment: {anchor, quote, el, sending}
	var box = null;
	var boxFor = null; // the pin element the open box sits next to

	function readPins() {
		var el = document.getElementById('hub-pins');
		try {
			var list = JSON.parse(el ? el.textContent : '[]');
			return Array.isArray(list) ? list : [];
		} catch (_) {
			return [];
		}
	}

	function make(tag, cls, text) {
		var el = document.createElement(tag);
		if (cls) el.className = cls;
		if (text != null) el.textContent = text;
		return el;
	}

	function shadowHost(tag, css) {
		var host = document.createElement(tag);
		host.setAttribute('data-hub', '');
		var root = host.attachShadow({ mode: 'open' });
		root.appendChild(make('style', '', css));
		return { host: host, root: root };
	}

	function post(msg) {
		if (framed) window.parent.postMessage(msg, '*');
	}

	function today() {
		return new Date().toLocaleDateString('en-CA');
	}

	// ---------------------------------------------------------------- anchors

	// The hub's own elements are left out, so they never shift an index.
	function elementsOf(root) {
		return Array.prototype.filter.call(root.querySelectorAll('*'), function (el) {
			return !el.hasAttribute('data-hub');
		});
	}

	function rootElement(name) {
		if (name === 'page') return pageRoot;
		if (name === 'scope') return document.querySelector('[data-scope]');
		if (name === 'shared') return document.querySelector('[data-shared]');
		var id = name.slice('claim-'.length);
		return Array.prototype.find.call(document.querySelectorAll('[data-claim]'), function (s) {
			return s.dataset.claim === id;
		}) || null;
	}

	// The root is looked up from the parent, so a tap on a claim section itself
	// is anchored in the page, where the section is a descendant.
	function rootOf(el) {
		var up = el.parentElement;
		var r = up.closest('[data-claim]');
		if (r) return { name: 'claim-' + r.dataset.claim, el: r };
		if ((r = up.closest('[data-scope]'))) return { name: 'scope', el: r };
		if ((r = up.closest('[data-shared]'))) return { name: 'shared', el: r };
		return { name: 'page', el: pageRoot };
	}

	// A tap on the page root's own gaps or margins goes to its nearest child.
	function nearestChild(y) {
		var best = null;
		var bestGap = Infinity;
		Array.prototype.forEach.call(pageRoot.children, function (c) {
			if (c.hasAttribute('data-hub') || c.tagName === 'SCRIPT' || c.tagName === 'STYLE') return;
			var r = c.getBoundingClientRect();
			var gap = y < r.top ? r.top - y : y > r.bottom ? y - r.bottom : 0;
			if (gap < bestGap) { best = c; bestGap = gap; }
		});
		return best;
	}

	function percent(v, start, size) {
		return size > 0 ? Math.min(100, Math.max(0, (v - start) / size * 100)) : 0;
	}

	function anchorFor(target, cx, cy) {
		var el = target && target.nodeType === 1 ? target : target && target.parentElement;
		if (!el || el === pageRoot || !pageRoot.contains(el)) el = nearestChild(cy);
		if (!el) return null;
		var root = rootOf(el);
		var r = el.getBoundingClientRect();
		return {
			anchor: formatAnchor({ root: root.name, n: elementsOf(root.el).indexOf(el), x: percent(cx, r.left, r.width), y: percent(cy, r.top, r.height) }),
			quote: clampQuote(el.textContent),
		};
	}

	// Where an anchor sits now, in viewport coordinates.
	function pointOf(anchor) {
		var a = parseAnchor(anchor);
		var r;
		if (a && a.decision) {
			var f = fieldsetOf(a.decision);
			if (f) { r = f.getBoundingClientRect(); return { x: r.right - 22, y: r.top }; }
		} else if (a) {
			var root = rootElement(a.root);
			if (root) {
				var el = elementsOf(root)[a.n];
				if (!el) { r = root.getBoundingClientRect(); return { x: r.left, y: r.top }; }
				r = el.getBoundingClientRect();
				return { x: r.left + r.width * a.x / 100, y: r.top + r.height * a.y / 100 };
			}
		}
		r = pageRoot.getBoundingClientRect();
		return { x: r.left, y: r.top };
	}

	// ------------------------------------------------------------------ layer

	var layer = shadowHost('div', LAYER_CSS);
	layer.host.style.cssText = 'position:absolute;left:0;top:0;width:0;height:0;z-index:2147483000';
	var pinBox = make('div');
	var bar = make('div', 'bar');
	bar.hidden = true;
	bar.appendChild(make('span', '', 'Tap anywhere to comment'));
	var done = make('button', '', 'Done');
	done.type = 'button';
	bar.appendChild(done);
	var chat = make('button', 'chat');
	chat.type = 'button';
	chat.setAttribute('aria-label', 'Comment');
	chat.setAttribute('aria-pressed', 'false');
	chat.innerHTML = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true">' +
		'<path d="M4 5h16a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1h-9l-5 4v-4H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/></svg>';
	layer.root.appendChild(pinBox);
	layer.root.appendChild(bar);
	layer.root.appendChild(chat);
	document.body.appendChild(layer.host);

	var cursor = make('style', '', 'html.hub-commenting,html.hub-commenting *{cursor:crosshair!important}');
	cursor.setAttribute('data-hub', '');
	document.head.appendChild(cursor);

	function drawPins() {
		pinBox.textContent = '';
		var perFieldset = {};
		pins.forEach(function (pin) {
			var a = parseAnchor(pin.anchor);
			var shift = 0;
			if (a && a.decision) {
				shift = (perFieldset[a.decision] || 0) * 32;
				perFieldset[a.decision] = (perFieldset[a.decision] || 0) + 1;
			}
			var el = make('button', 'pin', 'S');
			el.type = 'button';
			el.dataset.anchor = pin.anchor;
			el.dataset.id = pin.id;
			el.dataset.shift = shift;
			el.setAttribute('aria-label', 'Comment: ' + clampQuote(pin.text));
			el.addEventListener('click', function () { openThread(pin, el); });
			pinBox.appendChild(el);
		});
		if (draft) {
			draft.el.dataset.anchor = draft.anchor;
			pinBox.appendChild(draft.el);
		}
		layout();
	}

	function layout() {
		var origin = layer.host.getBoundingClientRect();
		var width = document.documentElement.clientWidth;
		Array.prototype.forEach.call(pinBox.children, function (el) {
			var p = pointOf(el.dataset.anchor);
			var x = Math.min(Math.max(p.x - (+el.dataset.shift || 0), 14), width - 14);
			el.style.left = (x - origin.left) + 'px';
			el.style.top = (p.y - origin.top) + 'px';
		});
		if (box && boxFor) placeBox();
	}

	// Next to the pin on a wide screen; a narrow one gets a bottom sheet in CSS.
	function placeBox() {
		var origin = layer.host.getBoundingClientRect();
		var p = boxFor.getBoundingClientRect();
		var w = box.offsetWidth || 300;
		var x = p.right + 8;
		if (x + w > window.innerWidth - 8) x = p.left - 8 - w;
		x = Math.max(8, x);
		box.style.left = (x - origin.left) + 'px';
		box.style.top = (Math.max(8, p.top - 10) - origin.top) + 'px';
	}

	var queued = false;
	function relayout() {
		if (queued) return;
		queued = true;
		requestAnimationFrame(function () { queued = false; layout(); });
	}
	window.addEventListener('resize', relayout);
	window.addEventListener('load', relayout);
	if (document.fonts && document.fonts.ready) document.fonts.ready.then(relayout);
	if (typeof ResizeObserver === 'function') new ResizeObserver(relayout).observe(pageRoot);

	// -------------------------------------------------------------------- box

	function openBox(pinEl, fill) {
		closeBox();
		box = make('div', 'box');
		box.setAttribute('role', 'dialog');
		var head = make('div', 'head');
		head.appendChild(make('span', 'title', document.title || 'This plan'));
		var x = make('button', 'x', '×');
		x.type = 'button';
		x.setAttribute('aria-label', 'Close');
		x.addEventListener('click', dismiss);
		head.appendChild(x);
		box.appendChild(head);
		fill(box);
		boxFor = pinEl;
		layer.root.appendChild(box);
		placeBox();
	}

	function closeBox() {
		if (box) box.remove();
		box = null;
		boxFor = null;
	}

	// Escape, Cancel and the close button: an unsent pin goes with its box.
	function dismiss() {
		if (draft && draft.sending) return;
		if (draft) { draft = null; drawPins(); }
		closeBox();
	}

	function message(into, who, cls, text) {
		var m = make('div', 'msg');
		m.appendChild(make('span', 'who' + (cls ? ' ' + cls : ''), who));
		m.appendChild(make('div', 'text', text));
		into.appendChild(m);
	}

	function openThread(pin, pinEl) {
		if (draft && draft.sending) return;
		if (draft) { draft = null; drawPins(); }
		openBox(pinEl, function (b) {
			message(b, 'Saiful' + (pin.at ? ' · ' + pin.at : ''), '', pin.text || '');
			if (pin.reply) message(b, 'Claude', 'claude', pin.reply);
			else b.appendChild(make('div', 'note', pin.queued ? 'Queued for Claude' : 'Note'));
		});
	}

	function openDraft(d) {
		if (box && boxFor === d.el) return;
		openBox(d.el, function (b) {
			var text = make('textarea');
			text.placeholder = 'Add a comment';
			text.setAttribute('aria-label', 'Comment');
			var row = make('div', 'row');
			var label = make('label');
			var queue = make('input');
			queue.type = 'checkbox';
			queue.checked = true;
			label.appendChild(queue);
			label.appendChild(document.createTextNode('Queue for Claude'));
			var cancel = make('button', 'cancel', 'Cancel');
			cancel.type = 'button';
			cancel.addEventListener('click', dismiss);
			row.appendChild(label);
			row.appendChild(cancel);
			var status = make('div', 'status');
			b.appendChild(text);
			b.appendChild(row);
			b.appendChild(status);
			d.text = text;
			if (!framed) {
				status.textContent = 'This page is open on its own, so there is nowhere to send a comment. Open it from the hub.';
				return;
			}
			var send = make('button', 'send', '↑');
			send.type = 'button';
			send.setAttribute('aria-label', 'Send');
			row.appendChild(send);
			// The hub sends only on a live tap, so the post happens inside the click.
			function submit() {
				var body = text.value.trim();
				if (!body || d.sending) return;
				post({ type: 'plan-comment', anchor: d.anchor, text: body, queue: queue.checked, quote: d.quote });
				d.sending = true;
				send.disabled = true;
				text.readOnly = true;
				status.className = 'status';
				status.textContent = 'Sending…';
				pending[d.anchor] = function (r) {
					if (r.ok) {
						var pin = { id: r.id, anchor: d.anchor, text: body, queued: queue.checked, reply: null, at: today() };
						pins.push(pin);
						draft = null;
						drawPins();
						var el = pinBox.querySelector('[data-id="' + r.id + '"]');
						if (el) openThread(pin, el); else closeBox();
						return;
					}
					d.sending = false;
					send.disabled = false;
					text.readOnly = false;
					status.className = 'status err';
					status.textContent = r.error || 'Not sent.';
				};
			}
			send.addEventListener('click', submit);
			text.addEventListener('keydown', function (e) {
				if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit();
			});
		});
		d.text.focus();
	}

	// ------------------------------------------------------------------ modes

	function setMode(on) {
		mode = on;
		bar.hidden = !on;
		chat.setAttribute('aria-pressed', String(on));
		document.documentElement.classList.toggle('hub-commenting', on);
		if (!on && draft && !draft.sending) dismiss();
		if (on && box && !draft) closeBox();
	}
	chat.addEventListener('click', function () { setMode(!mode); });
	done.addEventListener('click', function () { setMode(false); });

	function fromHub(e) {
		return e.composedPath().some(function (n) { return n.nodeType === 1 && n.hasAttribute('data-hub'); });
	}

	// Capture phase: with the mode on, no link, card or radio sees the tap.
	document.addEventListener('click', function (e) {
		if (fromHub(e)) return;
		if (!mode) {
			if (box && !draft) closeBox();
			return;
		}
		e.preventDefault();
		e.stopPropagation();
		if (draft && (draft.sending || draft.text.value.trim())) { draft.text.focus(); return; }
		var a = anchorFor(e.target, e.clientX, e.clientY);
		if (!a) return;
		var el = make('button', 'pin draft', 'S');
		el.type = 'button';
		el.setAttribute('aria-label', 'New comment');
		draft = { anchor: a.anchor, quote: a.quote, el: el, sending: false };
		el.addEventListener('click', function () { if (draft) openDraft(draft); });
		drawPins();
		openDraft(draft);
	}, true);

	document.addEventListener('keydown', function (e) {
		if (e.key !== 'Escape') return;
		if (box) dismiss();
		else if (mode) setMode(false);
	});

	// -------------------------------------------------------------- decisions

	var baseline = {}; // decision name -> the value the page counts as decided

	function fieldsetOf(name) {
		return fieldsets.find(function (f) { return f.dataset.decision === name; }) || null;
	}

	function radios(f) {
		return Array.prototype.filter.call(f.querySelectorAll('input[type="radio"]'), function (i) {
			return i.name === f.dataset.decision;
		});
	}

	function optionLabel(input) {
		var ol = input && input.closest('label') && input.closest('label').querySelector('.ol');
		if (!ol) return input ? input.value : '';
		var copy = ol.cloneNode(true);
		Array.prototype.forEach.call(copy.querySelectorAll('em'), function (em) { em.remove(); });
		return clampQuote(copy.textContent);
	}

	function markSeen(name) {
		if (seen[name]) return;
		seen[name] = true;
		postDraft();
		postState();
	}

	function postDraft() {
		if (started) post({ type: 'plan-draft', state: JSON.stringify({ seen: seen }) });
	}

	function postState() {
		if (started) post({ type: 'plan-state', unopened: unopenedCount(names, seen, pins) });
	}

	fieldsets.forEach(function (f) {
		var name = f.dataset.decision;
		var inputs = radios(f);
		var first = inputs.find(function (i) { return i.defaultChecked; });
		baseline[name] = first ? first.value : null;
		pins.forEach(function (p) {
			var a = parseAnchor(p.anchor);
			if (a && a.decision === name) baseline[name] = a.value;
		});
		inputs.forEach(function (i) { if (i.value === baseline[name]) i.checked = true; });

		var ui = shadowHost('div', CHANGE_CSS);
		ui.host.hidden = true;
		var row = make('div', 'change');
		var status = make('span');
		ui.root.appendChild(row);
		f.appendChild(ui.host);

		var send = null;
		if (framed) {
			send = make('button', '', 'Send change');
			send.type = 'button';
			row.appendChild(send);
		} else {
			status.textContent = 'Open this plan from the hub to send a change.';
		}
		row.appendChild(status);

		function update() {
			var c = inputs.find(function (i) { return i.checked; });
			ui.host.hidden = !c || c.value === baseline[name];
		}

		f.addEventListener('change', function () { markSeen(name); update(); });
		f.addEventListener('click', function (e) {
			if (e.target.closest && e.target.closest('label, input')) markSeen(name);
		});

		if (send) send.addEventListener('click', function () {
			var c = inputs.find(function (i) { return i.checked; });
			if (!c || send.disabled) return;
			var was = inputs.find(function (i) { return i.value === baseline[name]; });
			var legend = f.querySelector('legend');
			var label = optionLabel(c);
			var anchor = 'decision-' + name + '@' + c.value;
			post({ type: 'plan-decision', name: name, value: c.value, label: label, was: optionLabel(was), question: clampQuote(legend ? legend.textContent : '') });
			send.disabled = true;
			status.className = '';
			status.textContent = 'Sending…';
			pending[anchor] = function (r) {
				send.disabled = false;
				if (r.ok) {
					baseline[name] = c.value;
					pins.push({ id: r.id, anchor: anchor, text: label, queued: true, reply: null, at: today() });
					status.textContent = '';
					drawPins();
					postState();
				} else {
					status.className = 'err';
					status.textContent = r.error || 'Not sent.';
				}
				update();
			};
		});
		update();
	});

	// ------------------------------------------------------------- messaging

	// A late restore still merges, so what the reader opened before it is kept.
	function restore(state) {
		try {
			var s = JSON.parse(state);
			if (s && s.seen) Object.keys(s.seen).forEach(function (k) { if (s.seen[k]) seen[k] = true; });
		} catch (_) { /* null or junk: start from nothing */ }
		started = true;
		postDraft();
		postState();
	}

	if (framed) {
		window.addEventListener('message', function (e) {
			var d = e.data;
			if (e.source !== window.parent || !d || typeof d !== 'object') return;
			if (d.type === 'plan-restore') restore(d.state);
			else if (d.type === 'plan-sent' && pending[d.anchor]) {
				var then = pending[d.anchor];
				delete pending[d.anchor];
				then(d);
			}
		});
		post({ type: 'plan-ready' });
		setTimeout(function () { if (!started) restore(null); }, 1500);
	}

	drawPins();
})();
