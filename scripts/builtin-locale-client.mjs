import {languages} from '../language-pack/src/register.mjs';
// Serialized into the no-import browser factory. Only public pinned SDK services
// are used; this coordinator owns exactly its own language/dictionary disposers.
function registerBuiltinLanguagePack(ctx, catalogs) {
  ctx.effect(() => {
    const modules = ctx.modules;
    const state = modules.entries.state;
    let stopped = false;
    let active = [];
    function release() {
      const previous = active;
      active = [];
      for (const dispose of previous.reverse()) dispose();
    }
    function reconcile() {
      // ClientEntries publishes syncing=true before updating its manifest or
      // activating new entries. Yield synchronously; inspecting the OLD graph
      // during that notification would race a newly enabled manual pack.
      const plugins = modules.manifest.plugins;
      if (stopped || state.getSnapshot().syncing || !Array.isArray(plugins) ||
          plugins.some(plugin => plugin.id === '@dsh-ocean/desktop-language-pack')) {
        release();
        return;
      }
      if (active.length) return;
      const occupied = new Set(ctx.locale.getLocale().locales.map(locale => locale.id.toLowerCase()));
      if (languages.some(language => occupied.has(language.id.toLowerCase()))) return;
      try {
        for (const language of languages) {
          active.push(ctx.locale.addLanguage(language));
          for (const [namespace, dictionary] of Object.entries(catalogs[language.id])) {
            active.push(ctx.locale.register(namespace, language.id, dictionary));
          }
        }
      } catch (error) {
        release();
        throw error;
      }
    }
    const unsubscribe = state.subscribe(reconcile);
    try { reconcile(); } catch (error) { unsubscribe(); throw error; }
    return () => { stopped = true; unsubscribe(); release(); };
  }, 'builtin-locales: managed graph ownership');
}
export function renderBuiltinClient(catalogs, moduleId = '@dsh-ocean/builtin-locales') {
  if (moduleId !== '@dsh-ocean/builtin-locales') throw new Error('invalid built-in module id');
  return `// Generated from scripts/builtin-locale-client.mjs and language-pack/data; do not edit.\nwindow.__ModuleLoader__.load({\n  id: '${moduleId}',\n  factory() {\n    const languages = ${JSON.stringify(languages)};\n    const catalogs = ${JSON.stringify(catalogs)};\n    ${registerBuiltinLanguagePack.toString()}\n    return { inject: ['locale', 'modules'], apply(ctx) { registerBuiltinLanguagePack(ctx, catalogs); } };\n  },\n});\n`;
}
