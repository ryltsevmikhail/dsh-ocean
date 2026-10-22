/** User-selectable languages, deliberately registered in the requested order. */
export const languages = [
  { id: 'ru', label: 'Русский', fallback: 'en' },
  { id: 'es', label: 'Español', fallback: 'en' },
  { id: 'pt-BR', label: 'Português (Brasil)', fallback: 'en' },
  { id: 'de', label: 'Deutsch', fallback: 'en' },
  { id: 'fr', label: 'Français', fallback: 'en' },
  { id: 'ja', label: '日本語', fallback: 'en' },
  { id: 'ko', label: '한국어', fallback: 'en' },
];

/** Register only official UI dictionaries; every resource belongs to Cordis. */
export function registerLanguagePack(ctx, catalogs) {
  for (const language of languages) {
    ctx.effect(() => ctx.locale.addLanguage(language), `desktop-language-pack: ${language.id}`);
    for (const [namespace, dictionary] of Object.entries(catalogs[language.id])) {
      ctx.effect(
        () => ctx.locale.register(namespace, language.id, dictionary),
        `desktop-language-pack: ${language.id}/${namespace}`,
      );
    }
  }
}
