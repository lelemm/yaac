// The only script (besides upstream Mermaid) permitted by the page nonce.
// The source is a base64 data attribute; it is never part of an executable script.
(async () => {
  const page = document.body;
  const source = new TextDecoder().decode(Uint8Array.from(atob(page.dataset.source), c => c.charCodeAt(0)));
  const dark = page.dataset.dark === 'true';
  const encode = value => new Promise(resolve => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result.split(',')[1]);
    reader.readAsDataURL(new Blob([value]));
  });
  try {
    const defaults = mermaid.mermaidAPI.getSiteConfig();
    const fixed = {
      startOnLoad: false, securityLevel: 'strict', htmlLabels: false, maxTextSize: 65536,
      flowchart: { htmlLabels: false }, fontFamily: 'UbuntuSans Nerd Font Propo',
      secure: [...defaults.secure, 'htmlLabels', 'securityLevel', 'dompurifyConfig', 'secure'],
    };
    mermaid.initialize(fixed);
    const {config} = await mermaid.parse(source);
    const authored = !!config.theme;
    if (!authored) mermaid.initialize({...fixed, theme: 'base', themeVariables: {
      darkMode: dark, background: dark ? '#16181d' : '#ffffff',
      primaryColor: dark ? '#22262e' : '#f7f8fa', primaryTextColor: dark ? '#e6e8ec' : '#1a1d23',
      primaryBorderColor: dark ? '#9aa2af' : '#5b6472', lineColor: dark ? '#c9ced6' : '#3a4050',
      edgeLabelBackground: dark ? '#22262e' : '#ffffff',
    }});
    await document.fonts.load('16px "UbuntuSans Nerd Font Propo"');
    const {svg} = await mermaid.render('diagram', source);
    const host = document.createElement('div');
    page.append(host);
    host.innerHTML = svg;
    const root = host.querySelector('svg');
    if ([...root.querySelectorAll('foreignObject')].some(n => n.textContent.trim() || n.querySelector('img')))
      throw Error('This diagram requires HTML labels.');
    const {x,y,width,height} = root.viewBox.baseVal;
    if (!(width > 0 && height > 0 && width <= 8192 && height <= 8192 && width*height <= 8000000))
      throw Error('Diagram exceeds the preview size.');
    // Freeze CSS in the browser before converting text to outlines. usvg does not implement
    // the browser cascade, CSS variables, or every selector emitted by upstream Mermaid.
    const properties = ['fill','fill-opacity','fill-rule','stroke','stroke-width','stroke-opacity',
      'stroke-dasharray','stroke-dashoffset','stroke-linecap','stroke-linejoin','opacity','color',
      'font-family','font-size','font-weight','font-style','text-anchor','dominant-baseline',
      'alignment-baseline','letter-spacing','visibility','display','marker-start','marker-mid','marker-end'];
    const elements = [root,...root.querySelectorAll('*')];
    const styles = elements.map(el => {
      const style = getComputedStyle(el);
      return properties.map(key => style.getPropertyValue(key));
    });
    elements.forEach((el,i) => properties.forEach((key,j) => {
      const value = styles[i][j].replace(/url\(["']?[^)"']*#([^)"']+)["']?\)/g, 'url(#$1)');
      if (key.startsWith('marker-')) {
        el.style.removeProperty(key);
        el.setAttribute(key,value);
      } else if (value) el.style.setProperty(key,value);
    }));
    root.querySelectorAll('style,foreignObject').forEach(el => el.remove());
    root.setAttribute('width', Math.ceil(width));
    root.setAttribute('height', Math.ceil(height));
    root.style.removeProperty('max-width');
    const bg = document.createElementNS(root.namespaceURI, 'rect');
    for (const [attr,value] of Object.entries({x,y,width,height,
      fill: mermaid.mermaidAPI.getConfig().themeVariables.background || (dark ? '#16181d' : '#ffffff')}))
      bg.setAttribute(attr,value);
    root.prepend(bg);
    page.textContent = 'YAAC_SVG:' + await encode(new XMLSerializer().serializeToString(root));
  } catch (error) {
    page.textContent = 'YAAC_ERROR:' + await encode(String(error.message || error).slice(0,500));
  }
})();
