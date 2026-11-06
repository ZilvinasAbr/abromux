#!/usr/bin/env python3
"""Make a copy of gpui 0.2.2 that can screenshot its own windows offscreen.

macOS won't give out window images while the screen is locked, and gpui
stops drawing occluded windows, so dev/gitlab/app.sh's screenshots stop
working when nobody's at the Mac. This copy, built only by
dev/gitlab/build.sh (abromux itself is untouched), changes nothing unless
GPUI_CAPTURE_DIR is set. With it set:

- frames are driven by a 30 Hz timer instead of the display link, and an
  occluded window keeps drawing;
- the Metal layer can be read back and gives up on a drawable after a
  second instead of blocking;
- when the file $GPUI_CAPTURE_DIR/request names a PNG path, the next frame
  is written there (via a temporary file and a rename) before it's shown.

usage: gpui-capture.py SRC_GPUI_DIR OUT_DIR
"""
import shutil, sys

src, out = sys.argv[1], sys.argv[2]
shutil.rmtree(out, ignore_errors=True)
shutil.copytree(src, out)


def patch(rel, pairs):
    p = f"{out}/{rel}"
    s = open(p).read()
    for a, b in pairs:
        if s.count(a) != 1:
            sys.exit(f"{rel}: expected one {a[:60]!r}, found {s.count(a)}")
        s = s.replace(a, b)
    open(p, "w").write(s)


CAPTURE = 'std::env::var_os("GPUI_CAPTURE_DIR").is_some()'

patch("src/platform/mac/display_link.rs", [
    ("    frame_requests: dispatch_source_t,\n}",
     "    frame_requests: dispatch_source_t,\n"
     "    ticker: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,\n}"),
    ("            let display_link = sys::DisplayLink::new(\n"
     "                display_id,\n"
     "                display_link_callback,\n"
     "                frame_requests as *mut c_void,\n"
     "            )?;\n",
     f"            let display_link = if {CAPTURE} {{ None }} else {{ Some(sys::DisplayLink::new(\n"
     "                display_id,\n"
     "                display_link_callback,\n"
     "                frame_requests as *mut c_void,\n"
     "            )?) };\n"),
    ("                display_link: Some(display_link),\n                frame_requests,\n",
     "                display_link,\n                frame_requests,\n                ticker: None,\n"),
    ("            self.display_link.as_mut().unwrap().start()?;\n",
     "            match self.display_link.as_mut() {\n"
     "                Some(l) => l.start()?,\n"
     "                None => {\n"
     "                    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));\n"
     "                    let (flag, source) = (alive.clone(), self.frame_requests as usize);\n"
     "                    std::thread::spawn(move || {\n"
     "                        while flag.load(std::sync::atomic::Ordering::Relaxed) {\n"
     "                            std::thread::sleep(std::time::Duration::from_millis(33));\n"
     "                            unsafe { dispatch_source_merge_data(source as dispatch_source_t, 1) };\n"
     "                        }\n"
     "                    });\n"
     "                    self.ticker = Some(alive);\n"
     "                }\n"
     "            }\n"),
    ("            self.display_link.as_mut().unwrap().stop()?;\n",
     "            if let Some(l) = self.display_link.as_mut() { l.stop()?; }\n"
     "            if let Some(t) = self.ticker.take() { t.store(false, std::sync::atomic::Ordering::Relaxed); }\n"),
])

patch("src/platform/mac/window.rs", [
    ("        unsafe {\n            if !self\n                .native_window\n                .occlusionState()",
     f"        unsafe {{\n            if !{CAPTURE} && !self\n                .native_window\n                .occlusionState()"),
    ("            lock.move_traffic_light();\n            lock.start_display_link();\n        } else {\n            lock.stop_display_link();",
     f"            lock.move_traffic_light();\n            lock.start_display_link();\n        }} else if !{CAPTURE} {{\n            lock.stop_display_link();"),
])

patch("src/platform/mac/metal_renderer.rs", [
    ("            let _: () = msg_send![&*layer, setAllowsNextDrawableTimeout: NO];\n",
     f"            let _: () = msg_send![&*layer, setAllowsNextDrawableTimeout: if {CAPTURE} {{ YES }} else {{ NO }}];\n"
     f"            if {CAPTURE} {{ layer.set_framebuffer_only(false); }}\n"),
    ("    pub fn draw(&mut self, scene: &Scene) {\n        let layer = self.layer.clone();\n",
     "    pub fn draw(&mut self, scene: &Scene) {\n        let capture = capture_request();\n        let layer = self.layer.clone();\n"),
    ("                    if self.presents_with_transaction {\n                        command_buffer.commit();",
     "                    if let Some(path) = capture {\n"
     "                        command_buffer.commit();\n"
     "                        command_buffer.wait_until_completed();\n"
     "                        save_texture(drawable.texture(), &path);\n"
     "                        drawable.present();\n"
     "                    } else if self.presents_with_transaction {\n                        command_buffer.commit();"),
])

with open(f"{out}/src/platform/mac/metal_renderer.rs", "a") as f:
    f.write('''
/// The PNG path in `$GPUI_CAPTURE_DIR/request`, taking the request.
fn capture_request() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("GPUI_CAPTURE_DIR")?);
    let req = dir.join("request");
    let path = std::fs::read_to_string(&req).ok()?;
    std::fs::remove_file(&req).ok()?;
    Some(path.trim().into())
}

fn save_texture(texture: &metal::TextureRef, path: &std::path::Path) {
    let (w, h) = (texture.width() as u32, texture.height() as u32);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    texture.get_bytes(
        buf.as_mut_ptr() as *mut std::ffi::c_void,
        (w * 4) as u64,
        metal::MTLRegion::new_2d(0, 0, w as u64, h as u64),
        0,
    );
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    let tmp = path.with_extension("tmp.png");
    if image::save_buffer(&tmp, &buf, w, h, image::ColorType::Rgba8).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}
''')
print(out)
