// One isolated local render. stdin is UTF-8 Mermaid, stdout is the two portable images.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const {pathToFileURL} = require('node:url');
const {randomBytes} = require('node:crypto');
const {execFileSync} = require('node:child_process');
const asset = (name,dev) => {
  const staged = path.join(__dirname,name);
  return fs.existsSync(staged) ? staged : path.resolve(__dirname,dev);
};
function browser() {
  const paths = process.platform === 'win32'
    ? ['ProgramFiles(x86)','ProgramFiles','LOCALAPPDATA'].flatMap(env => ['Microsoft/Edge','Google/Chrome']
        .map(dir => path.join(process.env[env] || '',dir,'Application',dir.startsWith('Microsoft')?'msedge.exe':'chrome.exe')))
    : ['/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/snap/bin/chromium'];
  const exe = [process.env.YAAC_MERMAID_BROWSER,...paths].find(p => p && fs.existsSync(p));
  if (!exe) throw Error('Mermaid needs a host installation of Edge or Chromium.');
  return exe;
}
// The outline converter cannot see browser font fallback. Supply host fonts for scripts
// outside the bundled face; their outlines travel with both exports.
function fallbackFonts(source) {
  const chars = [...new Set([...source].filter(c => c.codePointAt(0) > 0x2ff))];
  if (!chars.length) return [];
  const files = new Set();
  if (process.platform === 'win32') {
    const fonts = path.join(process.env.WINDIR,'Fonts');
    const add = file => { const p = path.join(fonts,file); if (fs.existsSync(p)) files.add(p); };
    add('segoeui.ttf');
    if (/[\p{Script=Hiragana}\p{Script=Katakana}]/u.test(source)) add('YuGothR.ttc');
    if (/\p{Script=Han}/u.test(source)) add('msyh.ttc');
    if (/\p{Script=Hangul}/u.test(source)) add('malgun.ttf');
    if (/\p{Script=Devanagari}/u.test(source)) add('Nirmala.ttf');
  } else {
    // A single Fontconfig query lists fallback faces in the host's preference order.
    const pattern = 'sans:charset=' + chars.map(c => c.codePointAt(0).toString(16)).join(' ');
    try {
      const matches = execFileSync('fc-match',['-s','--format','%{file}\n',pattern],{encoding:'utf8',timeout:3000});
      for (const file of matches.trim().split('\n').slice(0,5)) files.add(file);
    } catch { /* The bundled faces still work on hosts without fontconfig. */ }
  }
  return [...files].map(p => fs.readFileSync(p));
}
async function main() {
  const source = fs.readFileSync(0);
  if (source.length > 65536) throw Error('Mermaid source exceeds 64 KiB.');
  const exe = browser();
  const mermaid = asset('mermaid.min.js','node_modules/mermaid/dist/mermaid.min.js');
  const font = asset('font.ttf','../../crates/yaac-app/assets/fonts/UbuntuSansNerdFontPropo-Regular.ttf');
  const bold = asset('bold.ttf','../../crates/yaac-app/assets/fonts/UbuntuSansNerdFontPropo-Bold.ttf');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(),'yaac-mermaid-'));
  try {
    const nonce = randomBytes(20).toString('hex');
    const html = path.join(dir,'page.html');
    const script = (name,file) => {
      fs.copyFileSync(file,path.join(dir,name));
      return `<script nonce="${nonce}" src="${name}"></script>`;
    };
    const policy = [
      "default-src 'none'", `script-src 'nonce-${nonce}'`, "style-src 'unsafe-inline'",
      "font-src data:", "img-src data:", "base-uri 'none'", "form-action 'none'",
    ].join('; ');
    const dark = process.argv[2] === 'dark';
    const fontData = fs.readFileSync(font).toString('base64');
    fs.writeFileSync(html, `<!DOCTYPE html><meta charset="utf-8">
      <meta http-equiv="Content-Security-Policy" content="${policy}">
      <style>@font-face{font-family:'UbuntuSans Nerd Font Propo';src:url(data:font/ttf;base64,${fontData})}</style>
      <body data-dark="${dark}" data-source="${source.toString('base64')}">
        ${script('engine.js',mermaid)}${script('layout.js',path.join(__dirname,'layout.js'))}
      </body>`);
    const dom = execFileSync(exe,['--headless','--disable-gpu','--disable-extensions','--disable-background-networking',
      '--no-first-run','--no-default-browser-check',`--user-data-dir=${path.join(dir,'profile')}`,
      '--virtual-time-budget=10000','--dump-dom',pathToFileURL(html).href],
      {timeout:25000,maxBuffer:12*1024*1024,windowsHide:true,stdio:['ignore','pipe','ignore']}).toString();
    const match = dom.match(/>YAAC_(SVG|ERROR):([A-Za-z0-9+/=]+)\s*<\/body>/);
    if (!match) throw Error('Chromium could not lay out this diagram.');
    const result = Buffer.from(match[2],'base64').toString();
    if (match[1]==='ERROR') throw Error(result);
    const {Resvg,initWasm} = require(asset('resvg.cjs','node_modules/@resvg/resvg-wasm/index.js'));
    await initWasm(fs.readFileSync(asset('index_bg.wasm','node_modules/@resvg/resvg-wasm/index_bg.wasm')));
    const family = 'UbuntuSans Nerd Font Propo';
    const renderer = new Resvg(result,{font:{fontBuffers:[fs.readFileSync(font),fs.readFileSync(bold),...fallbackFonts(source.toString())],
      defaultFontFamily:family,sansSerifFamily:family}});
    if (renderer.width > 8192 || renderer.height > 8192 || renderer.width * renderer.height > 8000000)
      throw Error('Diagram exceeds the preview size.');
    const svg = renderer.toString();
    const png = Buffer.from(renderer.render().asPng());
    process.stdout.write(JSON.stringify({svg:Buffer.from(svg).toString('base64'),png:png.toString('base64')}));
  } finally {
    fs.rmSync(dir,{force:true,recursive:true,maxRetries:3});
  }
}
main().catch(e => {process.stderr.write(String(e.message).slice(0,800)); process.exitCode=1;});
