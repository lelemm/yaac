// Rebuilds index.html: the owner's revision 5 on top (with a before/after where there is one),
// then the earlier reviewed set for reference. Run: node build-gallery.mjs
import fs from 'node:fs';
const brief = fs.readFileSync('../screens.md', 'utf8');
const titles = {}, purpose = {};
for (const m of brief.matchAll(/^### (\d\d) — (.+)$/gm)) titles[m[1]] = m[2];
for (const m of brief.matchAll(/^### (\d\d) — .+\n\n\*(.+?)\*\n/gms)) purpose[m[1]] = m[2].replace(/\n/g, ' ');
const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;');
const list = dir => fs.existsSync(dir) ? fs.readdirSync(dir).filter(f => f.endsWith('-dark.png')).sort() : [];
const fig = (src, cap) => `<figure><a href="${src}"><img loading="lazy" src="${src}" alt="${esc(cap)}"></a><figcaption>${esc(cap)}</figcaption></figure>`;
const dirOf = {}; for (const d of ['v5', 'v6']) for (const f of list(d)) dirOf[f] = d;
const v5 = Object.keys(dirOf).sort(), old = list('final');
const beforeOf = n => old.find(f => f.startsWith(n + '-'));
const sec5 = v5.map(f => { const n = f.slice(0, 2), light = f.replace('-dark', '-light'), b = beforeOf(n);
  return `<section id="r${n}"><h2><span>${n}</span> ${esc(titles[n] || f)}</h2><p>${esc(purpose[n] || '')}</p>
<div class="pair">${fig(dirOf[f] + '/' + f, 'Now — dark')}${fig(dirOf[f] + '/' + light, 'Now — light')}</div>
${b ? `<details><summary>Before (the version you found polluted)</summary><div class="pair">${fig('final/' + b, 'Before — dark')}${fig('final/' + b.replace('-dark', '-light'), 'Before — light')}</div></details>` : ''}</section>`; }).join('\n');
const secOld = old.filter(f => !v5.some(g => g.slice(0, 2) === f.slice(0, 2)) && !['06', '18'].includes(f.slice(0, 2))).map(f => { const n = f.slice(0, 2);
  return `<section id="o${n}"><h2><span>${n}</span> ${esc(titles[n] || f)}</h2><p>${esc(purpose[n] || '')}</p>
<div class="pair">${fig('final/' + f, 'Earlier style — dark')}${fig('final/' + f.replace('-dark', '-light'), 'Earlier style — light')}</div></section>`; }).join('\n');
const shots = fs.existsSync('../screenshots') ? fs.readdirSync('../screenshots').filter(f => f.endsWith('-dark.png')).sort() : [];
const secShots = shots.map(f => { const n = f.slice(0, 2);
  return `<section id="a${n}"><h2><span>${n}</span> ${esc(titles[n] || f)}</h2>
<div class="pair">${fig('../screenshots/' + f, 'Real app — dark')}${fig('../screenshots/' + f.replace('-dark', '-light'), 'Real app — light')}</div></section>`; }).join('\n');
const nav = v5.map(f => `<a href="#r${f.slice(0, 2)}">${f.slice(0, 2)} ${esc(titles[f.slice(0, 2)] || '')}</a>`).join('');
fs.writeFileSync('index.html', `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>YAAC mockups</title><style>
:root{--bg:#f5f6f8;--fg:#1a1d23;--mut:#5b6472;--card:#fff;--bd:#dadde3;--ac:#2f4ac0}
@media(prefers-color-scheme:dark){:root{--bg:#16181d;--fg:#e6e8ec;--mut:#9aa2af;--card:#1b1e24;--bd:#2e333d;--ac:#8fa8ff}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}
header,.band{padding:24px 32px 8px}h1{margin:0 0 4px;font-size:22px}header p,.band p{margin:0 0 4px;color:var(--mut);max-width:90ch}
.band{margin-top:40px;border-top:1px solid var(--bd)}.band h1{font-size:18px}
nav{position:sticky;top:0;background:var(--bg);border-bottom:1px solid var(--bd);padding:10px 32px;display:flex;flex-wrap:wrap;gap:6px 14px;font-size:13px;z-index:1}
nav a{color:var(--ac);text-decoration:none;white-space:nowrap}nav a:hover{text-decoration:underline}
section{padding:28px 32px 8px;scroll-margin-top:70px}h2{margin:0 0 4px;font-size:18px}h2 span{color:var(--mut);font-weight:400;margin-right:6px}
section p{margin:0 0 12px;color:var(--mut);max-width:90ch}
.pair{display:grid;grid-template-columns:1fr 1fr;gap:16px}@media(max-width:1100px){.pair{grid-template-columns:1fr}}
figure{margin:0;background:var(--card);border-radius:6px;overflow:hidden}
img{display:block;width:100%;height:auto}figcaption{padding:6px 10px;font-size:12px;color:var(--mut)}
details{margin-top:12px}summary{cursor:pointer;color:var(--ac);font-size:13px;margin-bottom:10px}
</style></head><body><header><h1>YAAC — screen mockups, revision 5</h1>
<p>Your revision: a flat sidebar list under sticky headers, no outlines, the conversation first, and the agents panel as a pill. ${v5.length} screens rendered so far, each in dark and light. Click an image for full size; open "Before" under a screen to compare.</p></header>
<nav>${nav}</nav>
<div class="band" style="margin-top:0;border-top:0"><h1>The real app</h1><p>Screenshots taken from the actual window, running on scripted demo data. ${shots.length} screens.</p></div>
${secShots}
<div class="band"><h1>Mockups, revision 5</h1><p>Generated images the app was built from.</p></div>
${sec5}
<div class="band"><h1>Not yet re-rendered in the new style</h1><p>These ${secOld ? 'screens' : ''} still show the earlier, busier style. Their content stands; their look will follow revision 5.</p></div>
${secOld}
</body></html>`);
console.log('gallery:', v5.length, 'revision-5 screens;', old.length, 'earlier');
