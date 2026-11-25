// SPDX-License-Identifier: EUPL-1.2 · Copyright (c) 2026 example.org
// Gemeinsame Kopfzeile: Navigation und Benutzermenü.
//   leoMenu.show(me, aktiv)   me = Antwort von /api/me, aktiv = 'bericht' | 'benutzer' | 'protokoll'
//   leoMenu.hide()
// Ereignisse am document (die Seite kann darauf reagieren):
//   'leo-mfa'     Klick auf „Zwei-Faktor-Anmeldung“ (sonst Wechsel zu /?zwei-faktor)
//   'leo-logout'  vor dem Abmelden; preventDefault() bricht ab (z. B. ungespeicherte Änderungen)
// Darstellung: Knopf #themebtn wechselt Automatisch → Hell → Dunkel, gespeichert in localStorage 'leo-theme'.
(() => {
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const nav = document.getElementById('mainnav');
  const um = document.getElementById('usermenu');
  let btn, panel;

  function close(focus) {
    if (!panel || panel.classList.contains('hidden')) return;
    panel.classList.add('hidden');
    btn.setAttribute('aria-expanded', 'false');
    if (focus) btn.focus();
  }
  function open() {
    panel.classList.remove('hidden');
    btn.setAttribute('aria-expanded', 'true');
    panel.querySelector('a, button')?.focus();
  }

  async function logout() {
    const ev = new Event('leo-logout', {cancelable: true});
    if (!document.dispatchEvent(ev)) return;
    try { await fetch('/api/logout', {method: 'POST', credentials: 'same-origin'}); } catch (e) {}
    try { localStorage.removeItem('bf-job'); } catch (e) {}
    location.href = '/';
  }

  function show(me, active) {
    const items = [{key: 'bericht', href: '/', label: 'Bericht'}];
    if (me.admin) items.push({key: 'benutzer', href: '/admin', label: 'Benutzer'}, {key: 'protokoll', href: '/protokoll', label: 'Protokoll'});
    nav.innerHTML = '<ul>' + items.map(i => `<li><a href="${i.href}"${i.key === active ? ' aria-current="page"' : ''}>${i.label}</a></li>`).join('') + '</ul>';
    nav.classList.remove('hidden');

    um.innerHTML = `
      <button type="button" aria-expanded="false" aria-controls="usermenu-panel" aria-label="Benutzermenü ${esc(me.user)}">
        <span class="avatar" aria-hidden="true">${esc((me.user || '?').charAt(0))}</span>
        <span class="name">${esc(me.user)}</span><span class="caret" aria-hidden="true">▼</span>
      </button>
      <div class="panel hidden" id="usermenu-panel">
        <div class="who">Angemeldet als<strong>${esc(me.user)}</strong>${me.admin ? 'Admin' : 'Benutzer'}</div>
        <a href="/?zwei-faktor" id="um-mfa">Zwei-Faktor-Anmeldung <span class="state${me.mfa ? ' on' : ''}">${me.mfa ? 'ein' : 'aus'}</span></a>
        <button type="button" id="um-logout">Abmelden</button>
      </div>`;
    um.classList.remove('hidden');
    btn = um.querySelector('button');
    panel = um.querySelector('.panel');
    btn.addEventListener('click', () => (panel.classList.contains('hidden') ? open() : close(false)));
    um.querySelector('#um-logout').addEventListener('click', logout);
    um.querySelector('#um-mfa').addEventListener('click', e => {
      const ev = new Event('leo-mfa', {cancelable: true});
      // Seite mit eigenem Zwei-Faktor-Bereich übernimmt, sonst normaler Link
      if (!document.dispatchEvent(ev)) { e.preventDefault(); close(false); }
    });
  }

  function setMfa(on) {
    const s = um.querySelector('#um-mfa .state');
    if (s) { s.textContent = on ? 'ein' : 'aus'; s.classList.toggle('on', on); }
  }

  function hide() {
    nav.classList.add('hidden'); um.classList.add('hidden');
    nav.innerHTML = ''; um.innerHTML = '';
  }

  // Schließen mit Esc, Klick daneben oder wenn der Fokus das Menü verlässt
  document.addEventListener('keydown', e => { if (e.key === 'Escape') close(true); });
  document.addEventListener('click', e => { if (panel && !um.contains(e.target)) close(false); });
  um.addEventListener('focusout', e => { if (panel && !um.contains(e.relatedTarget)) close(false); });

  // --- Darstellung hell/dunkel ---
  const THEMES = [
    {key: 'auto', label: 'Automatisch', ico: '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="9"/><path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor"/></svg>'},
    {key: 'light', label: 'Hell', ico: '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>'},
    {key: 'dark', label: 'Dunkel', ico: '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/></svg>'},
  ];
  const tb = document.getElementById('themebtn');
  function currentTheme() {
    try { const t = localStorage.getItem('leo-theme'); return t === 'light' || t === 'dark' ? t : 'auto'; } catch (e) { return 'auto'; }
  }
  function applyTheme(key) {
    if (key === 'auto') delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = key;
    try { if (key === 'auto') localStorage.removeItem('leo-theme'); else localStorage.setItem('leo-theme', key); } catch (e) {}
    const i = THEMES.findIndex(t => t.key === key), t = THEMES[i], next = THEMES[(i + 1) % THEMES.length];
    tb.innerHTML = `<span class="ico">${t.ico}</span><span class="lbl">${t.label}</span>`;
    const hint = key === 'auto' ? 'wie im Betriebssystem eingestellt' : '';
    tb.setAttribute('aria-label', `Darstellung: ${t.label}${hint ? ' (' + hint + ')' : ''}. Wechseln zu ${next.label}`);
    tb.title = `Darstellung: ${t.label}${hint ? ' – ' + hint : ''}. Klicken für ${next.label}.`;
  }
  if (tb) {
    applyTheme(currentTheme());
    tb.addEventListener('click', () => {
      const i = THEMES.findIndex(t => t.key === currentTheme());
      applyTheme(THEMES[(i + 1) % THEMES.length].key);
    });
  }
  // in einem anderen Tab umgestellt: hier übernehmen
  window.addEventListener('storage', e => { if (e.key === 'leo-theme' && tb) applyTheme(currentTheme()); });

  window.leoMenu = {show, hide, setMfa};
})();
