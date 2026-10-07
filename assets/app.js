(() => {
  const form = document.querySelector('form.ruling');
  const article = document.querySelector('article.finding');
  const typing = (el) => el && (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT');
  const fieldFor = { dismiss: 'reason', escalate: 'escalate_to' };

  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey) && form) {
      ev.preventDefault();
      form.requestSubmit();
      return;
    }
    if (typing(document.activeElement) || ev.ctrlKey || ev.metaKey || ev.altKey) return;
    const radio = form && form.querySelector(`input[data-key="${ev.key}"]`);
    if (radio) {
      ev.preventDefault();
      radio.checked = true;
      const field = fieldFor[radio.value];
      if (field) form.elements[field].focus();
      return;
    }
    if (article && (ev.key === 'j' || ev.key === 'k')) {
      const to = article.dataset[ev.key === 'j' ? 'next' : 'prev'];
      if (to) location.href = to;
    }
  });

  if (window.mermaid) {
    const dark = matchMedia('(prefers-color-scheme: dark)').matches;
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default' });
    (async () => {
      for (const el of document.querySelectorAll('pre.mermaid')) {
        try {
          await mermaid.parse(el.textContent);
        } catch (e) {
          el.classList.remove('mermaid');
          const msg = document.createElement('p');
          msg.className = 'error';
          msg.textContent = 'This diagram could not be rendered: ' + (e && e.message ? e.message : e);
          el.before(msg);
        }
      }
      await mermaid.run({ querySelector: 'pre.mermaid' });
    })();
  }
})();
