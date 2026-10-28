// pleamar-wm remote · the page's service worker (push.rs): it shows the
// notices the server sends —encrypted for this phone; the browser has already
// decrypted them— and, touched, opens the caravan on the companion.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', e => e.waitUntil(self.clients.claim()));
self.addEventListener('push', e => {
  let n = {};
  try { n = e.data ? e.data.json() : {}; } catch (_) { n = { title: e.data ? e.data.text() : '' }; }
  e.waitUntil(self.registration.showNotification(n.title || 'pleamar', {
    body: n.body || '', tag: n.tag || '', renotify: !!n.tag, icon: '/icon-192.png', badge: '/icon-192.png',
    data: { url: n.url || '/caravan' },
  }));
});
self.addEventListener('notificationclick', e => {
  e.notification.close();
  const url = new URL((e.notification.data && e.notification.data.url) || '/caravan', self.location.origin).href;
  e.waitUntil(self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then(list => {
    for (const c of list) {
      if (new URL(c.url).pathname.startsWith('/caravan') && 'focus' in c) return c.navigate(url).then(w => (w || c).focus());
    }
    return self.clients.openWindow(url);
  }));
});
