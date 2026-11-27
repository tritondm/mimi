// SPDX-License-Identifier: EUPL-1.2 · Copyright (c) 2026 Andreas Kraxner
// Sprache der Oberfläche: Deutsch (Standard) oder Englisch, gespeichert im Cookie „mimi-lang“.
// Ohne Cookie gilt die Standardsprache des Servers (DEFAULT_LANG, hier eingesetzt).
// Vor menu.js und dem Seitenskript einbinden, am Ende von <body>.
//   L('Deutsch', 'English')   Text in der aktuellen Sprache
//   LANG                      'de' oder 'en'
//   LOCALE                    für Datum und Zahlen ('de-AT' oder 'en-GB')
// Feste Texte im HTML: data-en="…" ersetzt den Inhalt (HTML erlaubt), data-en-label das
// aria-label, data-en-title den title, data-en-placeholder den Platzhalter.
// Knopf #langbtn in der Kopfzeile schaltet um (lädt die Seite neu).
(() => {
  const DEFAULT = '__MIMI_DEFAULT_LANG__' === 'en' ? 'en' : 'de';
  const m = document.cookie.match(/(?:^|;\s*)mimi-lang=(de|en)(?:;|$)/);
  const lang = m ? m[1] : DEFAULT;
  const en = lang === 'en';
  window.LANG = lang;
  window.LOCALE = en ? 'en-GB' : 'de-AT';
  window.L = (de, e) => (en ? e : de);
  document.documentElement.lang = lang;

  if (en) {
    document.querySelectorAll('[data-en]').forEach(e => { e.innerHTML = e.dataset.en; });
    document.querySelectorAll('[data-en-label]').forEach(e => e.setAttribute('aria-label', e.dataset.enLabel));
    document.querySelectorAll('[data-en-title]').forEach(e => { e.title = e.dataset.enTitle; });
    document.querySelectorAll('[data-en-placeholder]').forEach(e => { e.placeholder = e.dataset.enPlaceholder; });
  }

  // Umschalter: zeigt die jeweils andere Sprache, in dieser Sprache beschriftet
  const b = document.getElementById('langbtn');
  if (b) {
    const other = en ? 'de' : 'en';
    b.innerHTML = `<span class="ico" aria-hidden="true"><svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" focusable="false"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/></svg></span><span class="lbl">${en ? 'Deutsch' : 'English'}</span>`;
    b.lang = other;
    b.setAttribute('aria-label', en ? 'Auf Deutsch umstellen' : 'Switch to English');
    b.title = b.getAttribute('aria-label');
    b.addEventListener('click', () => {
      document.cookie = `mimi-lang=${other}; Path=/; Max-Age=31536000; SameSite=Lax${location.protocol === 'https:' ? '; Secure' : ''}`;
      location.reload();
    });
  }
})();
