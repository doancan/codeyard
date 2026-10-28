(() => {
  const interval = Math.min(60000, Math.max(250, Number(document.currentScript?.dataset.interval) || 1000));
  let revision = null;
  let stopped = false;
  addEventListener('pagehide', () => { stopped = true; });
  addEventListener('pageshow', event => { if (event.persisted) { stopped = false; poll(); } });
  async function poll() {
    if (stopped) return;
    try {
      const suffix = revision === null ? '' : '?since=' + revision;
      const response = await fetch('/__codeyard_changes' + suffix, {cache: 'no-store'});
      if (!response.ok) throw new Error('Preview server unavailable');
      const update = await response.json();
      if (revision !== null && update.revision !== revision) {
        const refreshed = new Set();
        if (update.cssOnly) {
          for (const link of document.querySelectorAll('link[rel="stylesheet"]')) {
            const url = new URL(link.href, location.href);
            if (url.origin === location.origin && update.paths.includes(url.pathname)) {
              url.searchParams.set('__codeyard_reload', update.revision);
              link.href = url.href;
              refreshed.add(url.pathname);
            }
          }
        }
        if (!update.cssOnly || !update.paths.every(path => refreshed.has(path))) { location.reload(); return; }
      }
      revision = update.revision;
    } catch (_) { /* The next poll recovers after a temporary connection loss. */ }
    if (!stopped) setTimeout(poll, interval);
  }
  poll();
})();
