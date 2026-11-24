// Runs inside the copied page, after its own script. Reads window.__SPEC and appends its JSON result in a pre element.
(() => {
  const SPEC = window.__SPEC;
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  const txt = e => (e ? norm(e.textContent) : '');
  const tags = (e, sel) => [...e.querySelectorAll(sel)].map(txt);
  const px = v => Math.round(parseFloat(v) * 100) / 100;
  const SKIP = 'script,style,.npanel,.hybg,.ctxl';
  const out = { data: {}, panes: {}, frame: {}, scrapes: {} };

  const resolve = specs => specs.flatMap(s => {
    if (s.pane != null) {
      const hit = [...document.querySelectorAll('.mk .pn')].filter(p => {
        const t = p.querySelector('.ptabs .pt');
        return t && txt(t).startsWith(s.pane);
      });
      const chosen = s.nth != null ? [hit[s.nth]].filter(Boolean) : hit.slice(0, 1);
      return s.within ? chosen.flatMap(p => [...p.querySelectorAll(s.within)]) : chosen;
    }
    return [...document.querySelectorAll('.mk ' + s.css)];
  });

  const measure = (roots, name, sel, check) => {
    for (const r of roots) {
      const el = [...(r.matches(sel) ? [r] : []), ...r.querySelectorAll(sel)].find(e => e.offsetHeight > 0 && e.offsetWidth > 0);
      if (!el) continue;
      const c = getComputedStyle(el);
      return {
        part: name, check, selector: sel,
        height: el.offsetHeight, width: el.offsetWidth,
        padding: [c.paddingTop, c.paddingRight, c.paddingBottom, c.paddingLeft].map(px),
        font_size: px(c.fontSize), line_height: c.lineHeight === 'normal' ? null : px(c.lineHeight),
      };
    }
    return null;
  };


  const btnKind = b => (b.classList.contains('pri') ? 'primary' : b.classList.contains('ghost') ? 'ghost' : 'plain');
  const detailOf = d => {
    if (!d) return null;
    const pairs = k => { const o = []; for (let i = 0; i + 1 < k.children.length; i += 2) o.push([txt(k.children[i]), txt(k.children[i + 1])]); return o; };
    const lead = d.children[0] && d.children[0].tagName === 'SPAN' ? d.children[0].cloneNode(true) : null;
    const faint = lead && lead.querySelector('.faint');
    const note = faint ? txt(faint) : '';
    if (faint) faint.remove();
    const tg = d.querySelector(':scope > .row .tgl');
    return {
      kv: [...d.querySelectorAll('.kv > div')].map(x => [txt(x.children[0]), txt(x.children[1])]),
      kvl: d.querySelector('.kvl') ? pairs(d.querySelector('.kvl')) : [],
      buttons: [...d.querySelectorAll(':scope > .row > .btn')].map(b => ({ text: txt(b), kind: btnKind(b) })),
      note: txt(d.querySelector(':scope > .row > .faint')) || note,
      lead: lead ? txt(lead) : '',
      caps: tags(d, '.caps .tg2'),
      perms: [...d.querySelectorAll('.pm2 > span')].map(s => [txt(s), s.nextElementSibling.classList.contains('y')]),
      toggle: tg ? tg.classList.contains('y') : null,
      pre: txt(d.querySelector('pre')),
    };
  };
  const paneBy = label => [...document.querySelectorAll('.mk .pn')].find(p => txt(p.querySelector('.ptabs .pt')).startsWith(label));
  const DET = (label, rowSel) => [...paneBy(label).querySelectorAll(rowSel)].map(r => {
    const n = r.nextElementSibling;
    return detailOf(n && n.matches('.xd,.lgx') ? n : null);
  });
  const DBG = () => {
    const out = {}; let sec = '';
    for (const c of paneBy('Debugger').querySelector('.side').children) {
      if (c.classList.contains('dss')) { sec = txt(c); out[sec] = []; continue; }
      if (!sec || !c.matches('.vrr,.stk,.bkp')) continue;
      const n = c.nextElementSibling, d = n && n.classList.contains('xd') ? detailOf(n) : null;
      out[sec].push({ text: txt(c), detail: d, dim: c.style.opacity !== '' });
    }
    return out;
  };

  // One string per block of text: inline spans (code colouring, <b>, <code>) fold into the block they sit in.
  const strings = roots => {
    const seen = new Set(), list = [], titles = [];
    for (const root of roots) {
      const blocks = new Map();
      const put = (node, piece) => {
        let p = node;
        while (p !== root && p.parentElement && getComputedStyle(p).display === 'inline') p = p.parentElement;
        if (!blocks.has(p)) blocks.set(p, []);
        blocks.get(p).push(piece);
      };
      const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
      for (let n; (n = w.nextNode());) {
        const p = n.nodeType === 3 ? n.parentElement : n;
        if (!p || p.closest('script,style') || (p.closest(SKIP) && !root.matches(SKIP))) continue;
        // Text the page does not draw (a closed row's details) is not text a scene must show.
        if (!(n.tagName === 'BR' ? p.parentElement || p : p).getClientRects().length) continue;
        if (n.nodeType === 3) put(p, n.nodeValue);
        else if (n.tagName === 'BR') put(p.parentElement || p, ' ');
      }
      for (const parts of blocks.values()) {
        const t = norm(parts.join(''));
        if (t && !seen.has(t)) { seen.add(t); list.push(t); }
      }
      root.querySelectorAll('[title]').forEach(e => {
        const t = norm(e.getAttribute('title'));
        if (t && !titles.includes(t)) titles.push(t);
      });
    }
    return { strings: list, titles };
  };

  const openAll = roots => roots.forEach(r => r.querySelectorAll('.xr,.lgl').forEach(x => x.classList.add('open')));

  const run = () => {
    for (const n of SPEC.data || []) {
      try { out.data[n] = eval(n); } catch (e) { out.data[n] = null; }
    }
    for (const [k, js] of Object.entries(SPEC.scrapes || {})) out.scrapes[k] = eval(js);
    const targets = SPEC.roots.map(r => ({ r, els: resolve(r.sel) }));
    const mk = [...document.querySelectorAll('.mk')];
    targets.push({ r: { slug: '*frame', defs: SPEC.frame_defs }, els: mk });
    for (const t of targets) t.sizes = t.r.defs.filter(d => !d[3]).map(d => measure(t.els, d[0], d[1], d[2]));
    for (const t of targets) t.text = strings(t.els);
    openAll(mk);
    for (const t of targets) {
      const after = t.r.defs.filter(d => d[3]).map(d => measure(t.els, d[0], d[1], d[2]));
      const sizes = [...t.sizes, ...after].filter(Boolean);
      const body = { found: t.els.length, sizes, ...t.text };
      if (t.r.slug === '*frame') out.frame = body; else out.panes[t.r.slug] = body;
    }
  };

  setTimeout(() => {
    try { run(); } catch (e) { out.error = String(e && e.stack || e); }
    const pre = document.createElement('pre');
    pre.id = 'probe-out';
    pre.textContent = JSON.stringify(out);
    document.body.appendChild(pre);
  }, 1800);
})();
