// Owned pages only. Catalogs arrive as IPC JSON, never executable HTML or script.
(() => {
  const languages = new Set(['en', 'ru', 'es', 'pt-BR', 'de', 'fr', 'ja', 'ko']);
  function format(template, args) {
    const used = new Set();
    const result = template.replace(/\{\{|\}\}|\{([a-z][a-z0-9_]*)\}|[{}]/g, (token, name) => {
      if (token === '{{') return '{';
      if (token === '}}') return '}';
      if (!name || !Object.hasOwn(args, name)) throw new Error('invalid message arguments');
      used.add(name);
      return String(args[name]); // Insert once; do not rescan or interpret markup.
    });
    if (Object.keys(args).length !== used.size) throw new Error('unexpected message arguments');
    return result;
  }
  window.OceanI18n = Object.freeze({ create() {
    let language = 'en';
    let messages = Object.create(null);
    return Object.freeze({
      get language() { return language; },
      update(payload) {
        if (!payload || !languages.has(payload.language) || !payload.messages ||
            typeof payload.messages !== 'object' || Array.isArray(payload.messages)) return false;
        const entries = Object.entries(payload.messages);
        if (entries.length > 4096 || entries.some(([key, value]) =>
            !/^[a-z][a-z0-9_.-]*$/.test(key) || typeof value !== 'string' || !value.trim() || value.length > 8192)) return false;
        const next = Object.create(null);
        for (const [key, value] of entries) next[key] = value;
        messages = next;
        language = payload.language;
        return true;
      },
      t(key, args = {}, fallback = key) {
        try { return format(messages[key] ?? fallback, args); }
        catch { return fallback; }
      },
    });
  } });
})();
