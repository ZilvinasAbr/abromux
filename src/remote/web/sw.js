// The phone app's service worker: shows abromux's pushes (src/remote/push.rs)
// and opens the terminal a notification is about when it's tapped.
"use strict";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("push", (e) => {
  let d = {};
  try { d = e.data ? e.data.json() : {}; } catch { d = { title: e.data && e.data.text() }; }
  const open = d.workspace
    ? `#/ws/${encodeURIComponent(d.workspace)}/${encodeURIComponent(d.surface || "")}`
    : "#/";
  e.waitUntil(
    self.registration.showNotification(d.title || "abromux", {
      body: d.body || "",
      tag: d.tag || undefined,
      icon: "/icon-192.png",
      badge: "/icon-192.png",
      data: { open },
    })
  );
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const open = (e.notification.data && e.notification.data.open) || "#/";
  e.waitUntil((async () => {
    const all = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    for (const c of all) {
      c.postMessage({ open });
      if ("focus" in c) return c.focus();
    }
    return self.clients.openWindow("/" + open);
  })());
});
