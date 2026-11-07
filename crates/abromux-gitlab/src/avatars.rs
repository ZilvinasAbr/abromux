//! User avatars, fetched once each with curl (abromux has no gpui HTTP
//! client). Until one arrives, or if it can't, GitLab's own fallback shows:
//! the name's initial on a color picked by user id.

use crate::{style::*, types::User};
use gpui::{prelude::*, *};
use std::{collections::HashMap, process::Command, sync::Arc};

enum Slot {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

#[derive(Default)]
pub struct Avatars {
    slots: HashMap<String, Slot>,
}

impl Avatars {
    /// Start fetching the avatars of `users` not asked for yet.
    pub fn want<'a, V: 'static>(
        &mut self,
        users: impl IntoIterator<Item = &'a User>,
        cx: &mut Context<V>,
        store: fn(&mut V) -> &mut Avatars,
    ) {
        for u in users {
            let Some(url) = u.avatar_url.clone().filter(|u| u.starts_with("http")) else {
                continue;
            };
            if self.slots.contains_key(&url) {
                continue;
            }
            self.slots.insert(url.clone(), Slot::Loading);
            cx.spawn(async move |this, cx| {
                let u = url.clone();
                let got = cx.background_spawn(async move { fetch(&u) }).await;
                this.update(cx, |v, cx| {
                    store(v).slots.insert(url, got.map_or(Slot::Failed, |i| Slot::Ready(Arc::new(i))));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn image(&self, u: &User) -> Option<Arc<Image>> {
        match self.slots.get(u.avatar_url.as_deref()?)? {
            Slot::Ready(i) => Some(i.clone()),
            _ => None,
        }
    }

    /// The avatar, with the user's name on hover as GitLab shows it.
    pub fn of(&self, u: &User, size: f32) -> AnyElement {
        let name: SharedString = if u.username.is_empty() {
            u.name.clone().into()
        } else {
            format!("{} @{}", u.name, u.username).into()
        };
        div()
            .id(("avatar", u.id as usize))
            .flex_none()
            .child(self.picture(u, size))
            .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(name.clone()).build(window, cx))
            .into_any_element()
    }

    fn picture(&self, u: &User, size: f32) -> AnyElement {
        if let Some(i) = self.image(u) {
            return img(i).size(px(size)).flex_none().rounded_full().into_any_element();
        }
        // GitLab's identicon fallback colors.
        const COLORS: [u32; 7] = [0x5c2b8c, 0x2b5c8c, 0x2b8c6e, 0x8c6e2b, 0x8c2b4e, 0x4e8c2b, 0x2b8c8c];
        let initial = u.name.chars().next().unwrap_or('?').to_uppercase().to_string();
        div()
            .size(px(size))
            .flex_none()
            .rounded_full()
            .bg(c(COLORS[u.id as usize % COLORS.len()]))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(size * 0.45))
            .text_color(c(TEXT))
            .child(initial)
            .into_any_element()
    }
}

fn fetch(url: &str) -> Option<Image> {
    let out = Command::new("/usr/bin/curl")
        .args(["-sSfL", "--max-time", "15", "--", url])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    image_from_bytes(out.stdout)
}

/// An image gpui can draw, by the bytes' signature (PNG, JPEG, GIF, WebP).
pub(crate) fn image_from_bytes(b: Vec<u8>) -> Option<Image> {
    let format = if b.starts_with(b"\x89PNG") {
        ImageFormat::Png
    } else if b.starts_with(&[0xff, 0xd8]) {
        ImageFormat::Jpeg
    } else if b.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else {
        return None;
    };
    Some(Image::from_bytes(format, b))
}
