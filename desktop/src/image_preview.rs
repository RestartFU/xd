//! Images attached to a draft: the tiles inside the composer and the full-window
//! viewer a tile opens. Clicking the image zooms it, the wheel zooms about the
//! pointer, a drag pans, the arrow keys pan or step through the images, and a
//! click anywhere outside it closes the viewer.

use std::io::Cursor;

use gpui::{ClickEvent, ImageFormat, KeyDownEvent, MouseMoveEvent};

use super::*;

const TILE_SIZE: f32 = 64.0;
const TILE_RADIUS: f32 = 8.0;
const VIEWER_RADIUS: f32 = 8.0;
const BACKDROP: u32 = 0x000000bf;
const CLOSE_BACKGROUND: u32 = 0x000000a6;
const CLOSE_HOVER_BACKGROUND: u32 = 0x000000cc;
const CLOSE_RING: u32 = 0xffffff33;
const NAVIGATION_HOVER_BACKGROUND: u32 = 0xffffff1a;
const MEDIA_FOREGROUND: u32 = 0xffffff;
const CAPTION_COLOR: u32 = 0xffffffcc;

const MAX_ZOOM: f32 = 8.0;
const CLICK_ZOOM: f32 = 2.0;
const KEY_ZOOM_STEP: f32 = 1.5;
const PAN_STEP: f32 = 40.0;
/// Further than this between press and release is a drag, not a click.
const DRAG_SLOP: f32 = 4.0;
const WHEEL_ZOOM_RATE: f32 = 0.002;
/// Holding control is how a trackpad pinch arrives, and it reports small deltas.
const PINCH_ZOOM_RATE: f32 = 0.01;
/// One wheel notch is three lines here, and about 100px in a browser, which is
/// the distance the zoom rates above are tuned for.
const WHEEL_LINE_HEIGHT: f32 = 32.0;
const CAPTION_LINE_HEIGHT: f32 = 16.0;
const CAPTION_GAP: f32 = 8.0;
const CHROME_MARGIN: f32 = 0.04;
/// Below this width the viewer gives the image the room the side buttons take.
const NARROW_WINDOW: f32 = 640.0;
const SIDE_BUTTON_ROOM: f32 = 96.0;
const UNAVAILABLE_WIDTH: f32 = 512.0;
const UNAVAILABLE_HEIGHT: f32 = 192.0;

/// A tile's thumbnail, keyed by the image and the size in device pixels. The
/// sampler has no mipmaps, so a full-size image drawn at 64px comes out as noise;
/// the tile draws a copy already scaled to the pixels it covers.
pub(super) type ComposerThumbnails = HashMap<(u64, u32), ThumbnailState>;

#[derive(Clone)]
pub(super) enum ThumbnailState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

/// Scale and center-crop a PNG to fill a square, as PNG bytes.
fn cover_thumbnail(png: &[u8], side: u32) -> Result<Vec<u8>, image::ImageError> {
    let thumbnail = image::load_from_memory_with_format(png, image::ImageFormat::Png)?
        .resize_to_fill(side, side, image::imageops::FilterType::CatmullRom)
        .into_rgba8();
    let mut encoded = Cursor::new(Vec::new());
    thumbnail.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

#[derive(Clone)]
struct PreviewImage {
    name: String,
    image: Arc<Image>,
    size: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    start: (f32, f32),
    scroll: (f32, f32),
}

/// Where the viewer draws the current image, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewerLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content_width: f32,
    content_height: f32,
}

/// The images as they were when the viewer opened. Like a dialog over the draft,
/// it does not change under the reader if another device edits that draft.
pub(super) struct ImagePreview {
    chat: String,
    images: Vec<PreviewImage>,
    index: usize,
    zoom: f32,
    scroll: (f32, f32),
    drag: Option<Drag>,
    suppress_click: bool,
    window: (f32, f32),
}

impl ImagePreview {
    fn new(chat: String, images: Vec<PreviewImage>, index: usize, window: (f32, f32)) -> Self {
        Self {
            chat,
            images,
            index,
            zoom: 1.0,
            scroll: (0.0, 0.0),
            drag: None,
            suppress_click: false,
            window,
        }
    }

    fn current(&self) -> Option<&PreviewImage> {
        self.images.get(self.index)
    }

    fn max_viewer_height(window_height: f32) -> f32 {
        (window_height * 0.86).min(window_height - 160.0).max(1.0)
    }

    fn max_viewer_width(window_width: f32) -> f32 {
        let room = if window_width >= NARROW_WINDOW {
            SIDE_BUTTON_ROOM
        } else {
            0.0
        };
        (window_width * 0.92 - room).max(1.0)
    }

    fn layout(&self) -> ViewerLayout {
        let (window_width, window_height) = self.window;
        let max_width = Self::max_viewer_width(window_width);
        let max_height = Self::max_viewer_height(window_height);
        let (content_width, content_height) = match self.current().and_then(|image| image.size) {
            Some((width, height)) if width > 0 && height > 0 => {
                let (width, height) = (width as f32, height as f32);
                let fit = (max_width / width).min(max_height / height).min(1.0);
                (width * fit * self.zoom, height * fit * self.zoom)
            }
            _ => (
                UNAVAILABLE_WIDTH.min(max_width),
                UNAVAILABLE_HEIGHT.min(max_height),
            ),
        };
        let width = content_width.min(max_width);
        let height = content_height.min(max_height);
        let column_height = height + CAPTION_GAP + CAPTION_LINE_HEIGHT;
        ViewerLayout {
            x: (window_width - width) / 2.0,
            y: (window_height - column_height) / 2.0,
            width,
            height,
            content_width,
            content_height,
        }
    }

    fn is_over_image(&self, (x, y): (f32, f32)) -> bool {
        let layout = self.layout();
        (layout.x..layout.x + layout.width).contains(&x)
            && (layout.y..layout.y + layout.height).contains(&y)
    }

    fn clamp_scroll(&mut self) {
        let layout = self.layout();
        self.scroll.0 = self
            .scroll
            .0
            .clamp(0.0, (layout.content_width - layout.width).max(0.0));
        self.scroll.1 = self
            .scroll
            .1
            .clamp(0.0, (layout.content_height - layout.height).max(0.0));
    }

    /// A resized window refits the image.
    fn fit_window(&mut self, window: (f32, f32)) {
        if self.window != window {
            self.window = window;
            self.reset_view();
        }
    }

    fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.scroll = (0.0, 0.0);
        self.drag = None;
    }

    fn navigate(&mut self, direction: isize) {
        let count = self.images.len() as isize;
        if count > 1 {
            self.index = (self.index as isize + direction).rem_euclid(count) as usize;
            self.reset_view();
        }
    }

    /// Zoom while keeping the image under `point` where it is, or the middle of
    /// the viewer without one. The viewer grows as it zooms and stays centered,
    /// so the point has to be measured against where the viewer ends up.
    fn change_zoom(&mut self, next: f32, point: Option<(f32, f32)>) {
        let next = next.clamp(1.0, MAX_ZOOM);
        let has_size = self.current().is_some_and(|image| image.size.is_some());
        if !has_size || next == self.zoom {
            return;
        }
        let before = self.layout();
        let (x, y) = point.unwrap_or((
            before.x + before.width / 2.0,
            before.y + before.height / 2.0,
        ));
        let anchor_x = (self.scroll.0 + x - before.x) / self.zoom;
        let anchor_y = (self.scroll.1 + y - before.y) / self.zoom;
        self.zoom = next;
        let after = self.layout();
        self.scroll = (
            anchor_x * next - (x - after.x),
            anchor_y * next - (y - after.y),
        );
        self.clamp_scroll();
    }

    fn toggle_zoom(&mut self, point: Option<(f32, f32)>) {
        let next = if self.zoom > 1.0 { 1.0 } else { CLICK_ZOOM };
        self.change_zoom(next, point);
    }

    /// `delta_y` is positive when scrolling up, which zooms in.
    fn wheel(&mut self, delta_y: f32, pinch: bool, point: (f32, f32)) {
        if delta_y == 0.0 {
            return;
        }
        let rate = if pinch {
            PINCH_ZOOM_RATE
        } else {
            WHEEL_ZOOM_RATE
        };
        self.change_zoom(self.zoom * (delta_y * rate).exp(), Some(point));
    }

    /// Whether the key moved a zoomed image. An image with no sideways overflow
    /// leaves left and right to step through the images.
    fn pan(&mut self, key: &str) -> bool {
        if self.zoom <= 1.0 {
            return false;
        }
        let layout = self.layout();
        let step = match key {
            "left" | "right" if layout.content_width <= layout.width => return false,
            "left" => (-PAN_STEP, 0.0),
            "right" => (PAN_STEP, 0.0),
            "up" => (0.0, -PAN_STEP),
            "down" => (0.0, PAN_STEP),
            _ => return false,
        };
        self.scroll = (self.scroll.0 + step.0, self.scroll.1 + step.1);
        self.clamp_scroll();
        true
    }

    fn key_down(&mut self, key: &str, held: bool) -> bool {
        if self.pan(key) {
            return true;
        }
        match key {
            "left" => self.navigate(-1),
            "right" => self.navigate(1),
            "enter" | "space" => {
                if !held {
                    self.toggle_zoom(None);
                }
            }
            "+" | "=" => self.change_zoom(self.zoom * KEY_ZOOM_STEP, None),
            "-" => self.change_zoom(self.zoom / KEY_ZOOM_STEP, None),
            "0" => self.change_zoom(1.0, None),
            _ => return false,
        }
        true
    }

    fn press(&mut self, position: (f32, f32)) {
        self.suppress_click = false;
        if self.zoom > 1.0 {
            self.drag = Some(Drag {
                start: position,
                scroll: self.scroll,
            });
        }
    }

    fn drag_to(&mut self, position: (f32, f32)) {
        let Some(drag) = self.drag else {
            return;
        };
        let (x, y) = (position.0 - drag.start.0, position.1 - drag.start.1);
        if x.hypot(y) > DRAG_SLOP {
            self.suppress_click = true;
        }
        self.scroll = (drag.scroll.0 - x, drag.scroll.1 - y);
        self.clamp_scroll();
    }

    fn caption(&self) -> String {
        let name = self.current().map_or("", |image| image.name.as_str());
        if self.images.len() > 1 {
            format!("{name} ({}/{})", self.index + 1, self.images.len())
        } else {
            name.to_owned()
        }
    }
}

fn point_tuple(point: Point<Pixels>) -> (f32, f32) {
    (f32::from(point.x), f32::from(point.y))
}

fn unavailable_image(colors: ThemeColors) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .text_center()
        .text_size(px(14.0))
        .text_color(rgb(colors.text))
        .child("Image unavailable.")
        .into_any_element()
}

fn viewer_button(
    id: &'static str,
    size: f32,
    icon_path: &'static str,
    icon_size: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .absolute()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .cursor_pointer()
        .child(icon(icon_path, icon_size, MEDIA_FOREGROUND))
}

impl XdDesktop {
    /// Where an attachment is in the draft now. A tile's position is from the
    /// last draw, and a synchronized draft or a paste may have moved it since.
    fn draft_attachment_index(&self, id: u64) -> Option<usize> {
        self.model
            .draft_attachments
            .iter()
            .position(|attachment| attachment.id == id)
    }

    pub(super) fn remove_draft_attachment(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.draft_attachment_index(id) {
            self.model.draft_attachments.remove(index);
            self.attachments_dirty = true;
            self.attachment_generation = self.attachment_generation.saturating_add(1);
            self.schedule_draft_sync(cx);
        }
        let focus = self.composer_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    /// Start any thumbnails the draft needs and forget the ones it no longer does.
    fn request_composer_thumbnails(&mut self, side: u32, cx: &mut Context<Self>) {
        let wanted = self
            .model
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.preview.is_some())
            .map(|attachment| (attachment.id, side))
            .collect::<HashSet<_>>();
        self.composer_thumbnails
            .retain(|key, _| wanted.contains(key));
        for attachment in &self.model.draft_attachments {
            let Some(image) = attachment.preview.clone() else {
                continue;
            };
            let key = (attachment.id, side);
            if self.composer_thumbnails.contains_key(&key) {
                continue;
            }
            // Unreadable, or a picture too large to be worth allocating.
            if attachment.size.is_none() {
                self.composer_thumbnails.insert(key, ThumbnailState::Failed);
                continue;
            }
            self.composer_thumbnails
                .insert(key, ThumbnailState::Loading);
            cx.spawn(async move |this, cx| {
                let thumbnail = cx
                    .background_spawn(async move { cover_thumbnail(&image.bytes, side) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    // The attachment may have been removed while it decoded.
                    if let Some(state) = this.composer_thumbnails.get_mut(&key) {
                        *state = match thumbnail {
                            Ok(bytes) => ThumbnailState::Ready(Arc::new(Image::from_bytes(
                                ImageFormat::Png,
                                bytes,
                            ))),
                            Err(_) => ThumbnailState::Failed,
                        };
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// The row of tiles above the composer's editor: images as thumbnails,
    /// then every other file as its name.
    pub(super) fn render_composer_attachments(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let side = (TILE_SIZE * window.scale_factor()).ceil() as u32;
        self.request_composer_thumbnails(side, cx);
        if self.model.draft_attachments.is_empty() {
            return None;
        }
        let tiles = self
            .model
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.preview.is_some())
            .enumerate()
            .map(|(index, attachment)| {
                let image = attachment.id;
                let thumbnail = self.composer_thumbnails.get(&(image, side));
                let content = match thumbnail {
                    Some(ThumbnailState::Ready(thumbnail)) => img(thumbnail.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Fill)
                        .rounded(px(TILE_RADIUS))
                        .into_any_element(),
                    Some(ThumbnailState::Failed) => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px_1()
                        .text_center()
                        .text_size(px(10.0))
                        .text_color(rgb(colors.muted))
                        .overflow_hidden()
                        .child(attachment.name.clone())
                        .into_any_element(),
                    Some(ThumbnailState::Loading) | None => div().into_any_element(),
                };
                div()
                    .relative()
                    .size(px(TILE_SIZE))
                    .flex_none()
                    .rounded(px(TILE_RADIUS))
                    .bg(rgb(colors.background))
                    .child(
                        div()
                            .id(("draft-image", index))
                            .debug_selector(move || format!("draft-image-{index}"))
                            .size_full()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_image_preview(image, window, cx);
                            }))
                            .child(content),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(TILE_RADIUS))
                            .border_1()
                            .border_color(rgb(colors.border)),
                    )
                    .child(
                        div()
                            .id(("remove-draft-image", index))
                            .debug_selector(move || format!("remove-draft-image-{index}"))
                            .absolute()
                            .top(px(4.0))
                            .right(px(4.0))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgba(CLOSE_RING))
                            .bg(rgba(CLOSE_BACKGROUND))
                            .shadow_sm()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgba(CLOSE_HOVER_BACKGROUND)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.remove_draft_attachment(image, window, cx);
                            }))
                            .child(icon(CLOSE_ICON, 14.0, MEDIA_FOREGROUND)),
                    )
            })
            .collect::<Vec<_>>();
        let files = self
            .model
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.preview.is_none())
            .enumerate()
            .map(|(index, attachment)| {
                self.render_draft_file(colors, index, attachment, cx)
                    .into_any_element()
            });
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px(px(8.0))
                .mb_2()
                .children(tiles)
                .children(files)
                .into_any_element(),
        )
    }

    /// A file waiting to be sent: its name and size, and a button to drop it.
    fn render_draft_file(
        &self,
        colors: ThemeColors,
        index: usize,
        attachment: &Attachment,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = attachment.id;
        div()
            .id(("draft-file", index))
            .debug_selector(move || format!("draft-file-{index}"))
            .h(px(36.0))
            .max_w(px(260.0))
            .min_w_0()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .pl(px(10.0))
            .pr(px(4.0))
            .rounded(px(TILE_RADIUS))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.background))
            .child(icon(FILE_ICON, 15.0, colors.muted))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(15.0))
                            .text_color(rgb(colors.text))
                            .child(attachment.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(13.0))
                            .text_color(rgb(colors.faint))
                            .child(file_size_label(attachment.bytes)),
                    ),
            )
            .child(
                div()
                    .id(("remove-draft-file", index))
                    .debug_selector(move || format!("remove-draft-file-{index}"))
                    .size(px(24.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.remove_draft_attachment(id, window, cx);
                    }))
                    .child(icon(CLOSE_ICON, 13.0, colors.muted)),
            )
    }

    fn open_image_preview(&mut self, image_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.model.selected_chat.clone() else {
            return;
        };
        let shown = self
            .model
            .draft_attachments
            .iter()
            .filter_map(|attachment| {
                Some((attachment.id, attachment.preview.clone()?, attachment))
            });
        let mut index = None;
        let mut images = Vec::new();
        for (position, (id, image, attachment)) in shown.enumerate() {
            if id == image_id {
                index = Some(position);
            }
            images.push(PreviewImage {
                name: attachment.name.clone(),
                image,
                size: attachment.size,
            });
        }
        let Some(index) = index else {
            return;
        };
        let viewport = window.viewport_size();
        let window_size = (f32::from(viewport.width), f32::from(viewport.height));
        self.image_preview = Some(ImagePreview::new(chat, images, index, window_size));
        // Closing goes back to the editor the tile belongs to, whatever the click
        // on the tile did to the focus.
        let composer = self.composer_input.read(cx).focus_handle(cx);
        self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        self.minimal_popup_previous_focus = Some(composer.downgrade());
        cx.notify();
    }

    pub(super) fn close_image_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.restore_minimal_popup_focus(window);
        cx.notify();
    }

    fn image_preview_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        let Some(preview) = &mut self.image_preview else {
            return;
        };
        if preview.key_down(&event.keystroke.key, event.is_held) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn render_image_preview(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        // Its chat closed, or another one was selected, under it.
        if self
            .image_preview
            .as_ref()
            .is_some_and(|preview| self.model.selected_chat.as_ref() != Some(&preview.chat))
        {
            self.close_image_preview(window, cx);
            return None;
        }
        let focus = self.minimal_popup_focus.clone();
        let viewport = window.viewport_size();
        let window_size = (f32::from(viewport.width), f32::from(viewport.height));
        let preview = self.image_preview.as_mut()?;
        preview.fit_window(window_size);
        let layout = preview.layout();
        let caption = preview.caption();
        let current = preview.current()?.clone();
        let (scroll_x, scroll_y) = preview.scroll;
        let several = preview.images.len() > 1;
        let dragging = preview.drag.is_some();
        let zoomed = preview.zoom > 1.0;
        let narrow = window_size.0 < NARROW_WINDOW;
        let margin_x = window_size.0 * CHROME_MARGIN;
        let margin_y = window_size.1 * CHROME_MARGIN;
        let cursor = match (zoomed, dragging) {
            (true, true) => CursorStyle::ClosedHand,
            (true, false) => CursorStyle::OpenHand,
            (false, _) => CursorStyle::PointingHand,
        };

        let mut image = None;
        if current.size.is_some() {
            image = Some(
                img(current.image)
                    .absolute()
                    .left(px(-scroll_x))
                    .top(px(-scroll_y))
                    .w(px(layout.content_width))
                    .h(px(layout.content_height))
                    .object_fit(gpui::ObjectFit::Fill)
                    .rounded(px(VIEWER_RADIUS))
                    .with_fallback(move || unavailable_image(colors)),
            );
        }

        let viewer = div()
            .id("image-preview-viewer")
            .debug_selector(|| "image-preview-viewer".into())
            .relative()
            .flex_none()
            .w(px(layout.width))
            .h(px(layout.height))
            .overflow_hidden()
            .rounded(px(VIEWER_RADIUS))
            .bg(rgb(colors.background))
            .shadow_2xl()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if let Some(preview) = &mut this.image_preview {
                        preview.press(point_tuple(event.position));
                        cx.notify();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(WHEEL_LINE_HEIGHT)).y;
                if let Some(preview) = &mut this.image_preview {
                    preview.wheel(
                        f32::from(delta),
                        event.modifiers.control,
                        point_tuple(event.position),
                    );
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let Some(preview) = &mut this.image_preview else {
                    return;
                };
                if !event.standard_click() || preview.suppress_click || event.click_count() > 1 {
                    return;
                }
                preview.toggle_zoom(event.mouse_position().map(point_tuple));
                cx.notify();
            }))
            .map(|viewer| match image {
                Some(image) => viewer.child(image),
                None => viewer.child(unavailable_image(colors)),
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(VIEWER_RADIUS))
                    .border_1()
                    .border_color(rgb(colors.border)),
            );

        let content = div()
            .id("image-preview-content")
            .flex()
            .flex_col()
            .items_center()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(viewer)
            .child(
                div()
                    .debug_selector(|| "image-preview-caption".into())
                    .mt(px(CAPTION_GAP))
                    .h(px(CAPTION_LINE_HEIGHT))
                    .max_w(px(ImagePreview::max_viewer_width(window_size.0)))
                    .truncate()
                    .text_size(px(12.0))
                    .line_height(px(CAPTION_LINE_HEIGHT))
                    .text_color(rgba(CAPTION_COLOR))
                    .child(caption),
            );

        let close = viewer_button("image-preview-close", 28.0, CLOSE_ICON, 16.0)
            .top(px(margin_y))
            .right(px(margin_x))
            .border_1()
            .border_color(rgba(CLOSE_RING))
            .bg(rgba(CLOSE_BACKGROUND))
            .shadow_sm()
            .hover(|style| style.bg(rgba(CLOSE_HOVER_BACKGROUND)))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_image_preview(window, cx);
            }));

        let navigation = |id: &'static str,
                          icon_path: &'static str,
                          direction: isize,
                          cx: &mut Context<Self>| {
            let button = viewer_button(id, 32.0, icon_path, 20.0)
                .hover(|style| style.bg(rgba(NAVIGATION_HOVER_BACKGROUND)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(preview) = &mut this.image_preview {
                        preview.navigate(direction);
                        cx.notify();
                    }
                }));
            let button = if direction < 0 {
                button.left(px(margin_x))
            } else {
                button.right(px(margin_x))
            };
            if narrow {
                button.bottom(px(margin_y))
            } else {
                button.top(px(window_size.1 / 2.0 - 16.0))
            }
        };
        let previous =
            several.then(|| navigation("image-preview-previous", CHEVRON_LEFT_ICON, -1, cx));
        let next = several.then(|| navigation("image-preview-next", CHEVRON_RIGHT_ICON, 1, cx));

        Some(
            div()
                .id("image-preview")
                .debug_selector(|| "image-preview".into())
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(BACKDROP))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    this.image_preview_key_down(event, cx);
                }))
                // The drag belongs to the whole window: it may leave the image.
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    let Some(preview) = &mut this.image_preview else {
                        return;
                    };
                    if preview.drag.is_none() {
                        return;
                    }
                    if event.dragging() {
                        preview.drag_to(point_tuple(event.position));
                    } else {
                        // Released outside the window, where nothing reported it.
                        preview.drag = None;
                    }
                    cx.notify();
                }))
                // In the capture phase, since the image's click stops the bubbling.
                .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                    if let Some(preview) = &mut this.image_preview
                        && preview.drag.take().is_some()
                    {
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    // Holding focus, the backdrop also gets a keyboard click when
                    // Enter or Space, which zoom, are released. And it sees a press
                    // on the image, so a drag let go out here arrives as a click.
                    let ClickEvent::Mouse(click) = event else {
                        return;
                    };
                    let began_on_image = this.image_preview.as_ref().is_some_and(|preview| {
                        preview.is_over_image(point_tuple(click.down.position))
                    });
                    if event.standard_click() && !began_on_image {
                        this.close_image_preview(window, cx);
                    }
                }))
                .child(content)
                .child(close)
                .children(previous)
                .children(next)
                .into_any_element(),
        )
    }
}

/// A file's size as the composer shows it: "812 B", "14 KB", "3.2 MB".
pub(super) fn file_size_label(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let size = bytes as f64;
    if size < KB {
        format!("{bytes} B")
    } else if size < KB * KB {
        format!("{:.0} KB", (size / KB).max(1.0))
    } else {
        let megabytes = size / (KB * KB);
        if megabytes < 10.0 {
            format!("{megabytes:.1} MB")
        } else {
            format!("{megabytes:.0} MB")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let pixels = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
        let mut encoded = Cursor::new(Vec::new());
        pixels
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("the test image encodes");
        encoded.into_inner()
    }

    fn preview(sizes: &[(u32, u32)], window: (f32, f32)) -> ImagePreview {
        let images = sizes
            .iter()
            .enumerate()
            .map(|(index, &size)| PreviewImage {
                name: format!("image-{index}.png"),
                image: Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())),
                size: Some(size),
            })
            .collect();
        ImagePreview::new("chat".into(), images, 0, window)
    }

    /// The window position of a point in the image, in fitted image coordinates.
    fn window_position(preview: &ImagePreview, image: (f32, f32)) -> (f32, f32) {
        let layout = preview.layout();
        (
            layout.x + image.0 * preview.zoom - preview.scroll.0,
            layout.y + image.1 * preview.zoom - preview.scroll.1,
        )
    }

    fn assert_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 0.01 && (actual.1 - expected.1).abs() < 0.01,
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn a_thumbnail_fills_its_square_from_the_middle_of_the_image() {
        // Red at both ends with blue in the middle: a stretched or letterboxed
        // thumbnail would have red in it, and the center crop has none.
        let source = png(400, 100, |x, _| {
            if (150..250).contains(&x) {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            }
        });

        let thumbnail = cover_thumbnail(&source, 64).expect("the thumbnail encodes");
        let decoded = image::load_from_memory(&thumbnail)
            .expect("the thumbnail decodes")
            .into_rgba8();

        assert_eq!(decoded.dimensions(), (64, 64));
        for point in [(0, 0), (63, 0), (0, 63), (63, 63), (32, 32)] {
            let [red, _, blue, _] = decoded.get_pixel(point.0, point.1).0;
            assert!(blue > 200 && red < 55, "{point:?} is {red},{blue}");
        }
    }

    #[test]
    fn a_thumbnail_of_something_that_is_not_an_image_fails() {
        assert!(cover_thumbnail(b"\x89PNG\r\n\x1a\nbroken", 64).is_err());
    }

    #[test]
    fn a_small_image_is_shown_at_its_own_size_and_a_large_one_fits_the_window() {
        let window = (1000.0, 800.0);
        let small = preview(&[(200, 100)], window).layout();
        assert_eq!((small.width, small.height), (200.0, 100.0));
        assert_eq!((small.x, small.y), (400.0, 338.0));

        let wide = preview(&[(4000, 1000)], window).layout();
        assert_eq!(wide.width, 1000.0 * 0.92 - SIDE_BUTTON_ROOM);
        assert!((wide.height - wide.width / 4.0).abs() < 0.01);

        let tall = preview(&[(1000, 4000)], window).layout();
        assert_eq!(tall.height, 640.0);
        assert_eq!(tall.width, 160.0);
    }

    #[test]
    fn a_narrow_window_gives_the_image_the_room_for_the_side_buttons() {
        let layout = preview(&[(4000, 1000)], (500.0, 800.0)).layout();
        assert_eq!(layout.width, 500.0 * 0.92);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_in_place() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        let layout = preview.layout();
        // A point well inside the image, so nothing clamps at either zoom.
        let image_point = (layout.width * 0.6, layout.height * 0.4);
        let pointer = window_position(&preview, image_point);

        preview.change_zoom(4.0, Some(pointer));
        assert_eq!(preview.zoom, 4.0);
        assert_close(window_position(&preview, image_point), pointer);

        preview.change_zoom(2.5, Some(pointer));
        assert_close(window_position(&preview, image_point), pointer);
    }

    #[test]
    fn zoom_stays_between_fit_and_the_maximum_and_fit_clears_the_scroll() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        preview.change_zoom(100.0, None);
        assert_eq!(preview.zoom, MAX_ZOOM);
        preview.change_zoom(0.1, None);
        assert_eq!(preview.zoom, 1.0);
        assert_eq!(preview.scroll, (0.0, 0.0));
    }

    #[test]
    fn scrolling_up_zooms_in_and_a_pinch_zooms_faster() {
        let mut wheel = preview(&[(4000, 3000)], (1400.0, 1000.0));
        wheel.wheel(96.0, false, (700.0, 500.0));
        assert!((wheel.zoom - (96.0 * WHEEL_ZOOM_RATE).exp()).abs() < 0.001);

        let mut pinch = preview(&[(4000, 3000)], (1400.0, 1000.0));
        pinch.wheel(96.0, true, (700.0, 500.0));
        assert!(pinch.zoom > wheel.zoom);

        let before = wheel.zoom;
        wheel.wheel(-96.0, false, (700.0, 500.0));
        assert!(wheel.zoom < before);
        wheel.wheel(0.0, false, (700.0, 500.0));
    }

    #[test]
    fn a_click_toggles_between_fit_and_double_size() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        preview.toggle_zoom(Some((700.0, 500.0)));
        assert_eq!(preview.zoom, CLICK_ZOOM);
        preview.change_zoom(5.0, None);
        preview.toggle_zoom(Some((700.0, 500.0)));
        assert_eq!(preview.zoom, 1.0);
    }

    #[test]
    fn arrows_step_through_images_until_zooming_gives_them_something_to_pan() {
        let mut preview = preview(&[(4000, 3000), (800, 600), (800, 600)], (1400.0, 1000.0));
        assert!(preview.key_down("left", false));
        assert_eq!(preview.index, 2, "stepping back wraps to the last image");
        assert!(preview.key_down("right", false));
        assert_eq!(preview.index, 0, "stepping on wraps to the first image");

        preview.change_zoom(3.0, None);
        let scroll = preview.scroll;
        assert!(preview.key_down("right", false));
        assert_eq!(preview.index, 0);
        assert_eq!(preview.scroll, (scroll.0 + PAN_STEP, scroll.1));
        assert!(preview.key_down("down", false));
        assert_eq!(preview.scroll, (scroll.0 + PAN_STEP, scroll.1 + PAN_STEP));
    }

    #[test]
    fn a_zoomed_image_with_no_sideways_overflow_still_steps_through_images() {
        // Tall enough to overflow downward at this zoom, narrow enough not to overflow sideways.
        let mut preview = preview(&[(300, 3000), (800, 600)], (1400.0, 1000.0));
        preview.change_zoom(2.0, None);
        let layout = preview.layout();
        assert!(layout.content_width <= layout.width);
        assert!(layout.content_height > layout.height);

        assert!(preview.key_down("right", false));
        assert_eq!(preview.index, 1);
    }

    #[test]
    fn stepping_to_another_image_refits_it() {
        let mut preview = preview(&[(4000, 3000), (800, 600)], (1400.0, 1000.0));
        preview.change_zoom(3.0, None);
        preview.navigate(1);
        assert_eq!(preview.zoom, 1.0);
        assert_eq!(preview.scroll, (0.0, 0.0));
    }

    #[test]
    fn a_single_image_has_nothing_to_step_to() {
        let mut preview = preview(&[(800, 600)], (1400.0, 1000.0));
        preview.key_down("right", false);
        assert_eq!(preview.index, 0);
        assert_eq!(preview.caption(), "image-0.png");
    }

    #[test]
    fn the_caption_counts_when_there_are_several() {
        let mut preview = preview(&[(800, 600), (800, 600)], (1400.0, 1000.0));
        assert_eq!(preview.caption(), "image-0.png (1/2)");
        preview.navigate(1);
        assert_eq!(preview.caption(), "image-1.png (2/2)");
    }

    #[test]
    fn zoom_keys_step_and_reset_and_a_held_enter_toggles_once() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        preview.key_down("=", false);
        assert_eq!(preview.zoom, KEY_ZOOM_STEP);
        preview.key_down("+", false);
        assert_eq!(preview.zoom, KEY_ZOOM_STEP * KEY_ZOOM_STEP);
        preview.key_down("-", false);
        assert_eq!(preview.zoom, KEY_ZOOM_STEP);
        preview.key_down("0", false);
        assert_eq!(preview.zoom, 1.0);

        preview.key_down("enter", false);
        assert_eq!(preview.zoom, CLICK_ZOOM);
        preview.key_down("enter", true);
        assert_eq!(preview.zoom, CLICK_ZOOM);
        preview.key_down("space", false);
        assert_eq!(preview.zoom, 1.0);
        assert!(!preview.key_down("a", false));
    }

    #[test]
    fn a_drag_pans_inside_the_image_and_suppresses_the_click_that_ends_it() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        preview.change_zoom(3.0, None);
        let scroll = preview.scroll;

        preview.press((500.0, 500.0));
        preview.drag_to((502.0, 501.0));
        assert!(!preview.suppress_click, "a shaky click is still a click");
        preview.drag_to((470.0, 490.0));
        assert!(preview.suppress_click);
        assert_close(preview.scroll, (scroll.0 + 30.0, scroll.1 + 10.0));

        preview.drag_to((-100_000.0, -100_000.0));
        let layout = preview.layout();
        assert_close(
            preview.scroll,
            (
                layout.content_width - layout.width,
                layout.content_height - layout.height,
            ),
        );

        preview.press((500.0, 500.0));
        assert!(!preview.suppress_click, "the next press starts fresh");
    }

    #[test]
    fn a_press_on_an_image_at_fit_does_not_start_a_drag() {
        let mut preview = preview(&[(800, 600)], (1400.0, 1000.0));
        preview.press((500.0, 500.0));
        assert!(preview.drag.is_none());
    }

    #[test]
    fn a_resized_window_refits_the_image() {
        let mut preview = preview(&[(4000, 3000)], (1400.0, 1000.0));
        preview.change_zoom(3.0, None);
        preview.fit_window((1400.0, 1000.0));
        assert_eq!(preview.zoom, 3.0, "the same size changes nothing");
        preview.fit_window((1200.0, 900.0));
        assert_eq!(preview.zoom, 1.0);
        assert_eq!(preview.scroll, (0.0, 0.0));
    }

    fn composer_with_images<'a>(
        cx: &'a mut gpui::TestAppContext,
        sizes: &[(u32, u32)],
    ) -> (Entity<XdDesktop>, &'a mut gpui::VisualTestContext) {
        install_embedded_fonts(cx.text_system()).unwrap();
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        let attachments = sizes
            .iter()
            .enumerate()
            .map(|(index, &(width, height))| {
                let shade = 40 * index as u8;
                let bytes = png(width, height, |_, _| [shade, 80, 160, 255]);
                Attachment::from_png(format!("image-{index}.png"), bytes).unwrap()
            })
            .collect::<Vec<_>>();
        desktop.update(cx, |desktop, _| {
            desktop.model = crate::tests::grouped_workspace_fixture();
            desktop.model.select_chat("implementation");
            desktop.model.backend = "codex".into();
            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "atlas".into(),
                chat_id: "implementation".into(),
                agent: AgentCli::Codex,
            };
            desktop.settings.browser_sessions.clear();
            desktop.transcript_loaded = true;
            desktop.transcript_loading = false;
            desktop.model.draft_attachments = attachments;
        });
        cx.simulate_resize(size(px(1200.0), px(800.0)));
        cx.run_until_parked();
        (desktop, cx)
    }

    fn click(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let target = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"))
            .center();
        cx.simulate_click(target, gpui::Modifiers::default());
        cx.run_until_parked();
    }

    fn composer_is_focused(desktop: &Entity<XdDesktop>, cx: &mut gpui::VisualTestContext) -> bool {
        cx.update(|window, cx| {
            desktop
                .read(cx)
                .composer_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        })
    }

    #[gpui::test]
    fn tiles_sit_inside_the_composer_with_their_thumbnails(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 200), (200, 800)]);

        let composer = cx.debug_bounds("chat-composer").unwrap();
        for selector in ["draft-image-0", "draft-image-1"] {
            let tile = cx.debug_bounds(selector).unwrap();
            assert_eq!(tile.size, size(px(TILE_SIZE), px(TILE_SIZE)));
            assert!(tile.left() > composer.left() && tile.right() < composer.right());
            assert!(tile.top() > composer.top() && tile.bottom() < composer.bottom());
        }
        let first = cx.debug_bounds("remove-draft-image-0").unwrap();
        let tile = cx.debug_bounds("draft-image-0").unwrap();
        assert!(first.right() < tile.right() && first.top() > tile.top());

        cx.update(|_, cx| {
            let thumbnails = &desktop.read(cx).composer_thumbnails;
            assert_eq!(thumbnails.len(), 2);
            assert!(
                thumbnails
                    .values()
                    .all(|state| matches!(state, ThumbnailState::Ready(_)))
            );
        });
    }

    #[gpui::test]
    fn files_sit_beside_the_images_and_open_the_viewer_only_on_images(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 200)]);
        desktop.update(cx, |desktop, cx| {
            desktop
                .model
                .draft_attachments
                .insert(0, Attachment::from_file("Report.pdf", vec![7; 3 * 1024]));
            desktop
                .model
                .draft_attachments
                .push(Attachment::from_file("notes.txt", b"hello".to_vec()));
            cx.notify();
        });
        cx.run_until_parked();

        let composer = cx.debug_bounds("chat-composer").unwrap();
        for selector in ["draft-file-0", "draft-file-1"] {
            let chip = cx.debug_bounds(selector).unwrap();
            assert!(chip.left() > composer.left() && chip.right() < composer.right());
            assert!(chip.top() > composer.top() && chip.bottom() < composer.bottom());
        }
        // The only image is the first tile, wherever it sits in the draft.
        click(cx, "draft-image-0");
        cx.update(|_, cx| {
            let preview = desktop
                .read(cx)
                .image_preview
                .as_ref()
                .expect("the viewer is open");
            assert_eq!(preview.caption(), "image-0.png");
        });
        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| desktop.close_image_preview(window, cx));
        });

        click(cx, "remove-draft-file-0");
        cx.update(|_, cx| {
            let desktop = desktop.read(cx);
            let names = desktop
                .model
                .draft_attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["image-0.png", "notes.txt"]);
            assert!(desktop.attachments_dirty);
        });
        assert!(composer_is_focused(&desktop, cx));
    }

    #[gpui::test]
    fn added_attachments_keep_to_the_hosts_limits(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(8, 8), (8, 8), (8, 8), (8, 8)]);
        desktop.update(cx, |desktop, _| {
            let image = desktop.model.draft_attachments[0].clone();
            assert!(
                desktop
                    .add_attachment(image)
                    .unwrap_err()
                    .contains("4 images")
            );
            let large = || Attachment::from_file("large.bin", vec![0; MAX_ATTACHMENT_BYTES]);
            desktop.add_attachment(large()).unwrap();
            assert!(
                desktop
                    .add_attachment(large())
                    .unwrap_err()
                    .contains("20 MiB")
            );
            for index in 0..5 {
                desktop
                    .add_attachment(Attachment::from_file(format!("{index}.txt"), vec![1]))
                    .unwrap();
            }
            assert_eq!(desktop.model.draft_attachments.len(), MAX_ATTACHMENTS);
            let extra = Attachment::from_file("extra.txt", vec![1]);
            assert!(
                desktop
                    .add_attachment(extra)
                    .unwrap_err()
                    .contains("10 attachments")
            );
        });
    }

    #[gpui::test]
    fn chosen_files_join_only_the_draft_they_were_chosen_for(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("xd-attach-chat-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let notes = root.join("notes.txt");
        fs::write(&notes, b"hello").unwrap();
        let (desktop, cx) = composer_with_images(cx, &[]);

        desktop.update(cx, |desktop, cx| {
            desktop.attach_paths("implementation".into(), vec![notes.clone()], cx);
            // Nothing is sent while a chosen file is still being read.
            assert_eq!(desktop.attachments_reading, 1);
            desktop.composer = "with the notes".into();
            desktop.model.connection_error = None;
            desktop.send_composer(cx);
            desktop.queue_composer(cx);
            desktop.resume_turn(cx);
            // With no host, a send, queue, or resume that went ahead would
            // have said so.
            assert_eq!(desktop.model.connection_error, None);
            // The user moves on before the read finishes.
            desktop.model.selected_chat = Some("elsewhere".into());
        });
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| {
            assert!(desktop.model.draft_attachments.is_empty());
            assert_eq!(desktop.attachments_reading, 0);
        });

        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("implementation".into());
            desktop.attach_paths("implementation".into(), vec![notes], cx);
        });
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| {
            let names = desktop
                .model
                .draft_attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["notes.txt"]);
            assert!(desktop.attachments_dirty);
        });
        fs::remove_dir_all(&root).unwrap();
    }

    #[gpui::test]
    fn a_failed_send_gives_back_its_files_beside_those_attached_since(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[]);
        desktop.update(cx, |desktop, cx| {
            let sent = Attachment::from_file("sent.pdf", b"%PDF".to_vec());
            let since = Attachment::from_file("since.txt", b"later".to_vec());
            desktop.pending_send = Some(PendingSend {
                chat_id: "implementation".into(),
                text: "read it".into(),
                attachments: vec![sent.clone()],
                restore: true,
                optimistic: OptimisticSend::Queued,
            });
            desktop.model.draft_attachments = vec![since, sent];
            desktop.restore_pending_send(cx);
            let names = desktop
                .model
                .draft_attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["sent.pdf", "since.txt"]);
            assert!(desktop.attachments_dirty);

            // Nothing joins the draft while a send is out.
            desktop.sending = true;
            let extra = Attachment::from_file("extra.txt", vec![1]);
            assert!(desktop.add_attachment(extra).unwrap_err().contains("Wait"));
            desktop.sending = false;

            // A restored draft keeps to the host's limits.
            let big =
                |name: &str, fill| Attachment::from_file(name, vec![fill; MAX_ATTACHMENT_BYTES]);
            desktop.pending_send = Some(PendingSend {
                chat_id: "implementation".into(),
                text: String::new(),
                attachments: vec![big("sent.bin", 1)],
                restore: true,
                optimistic: OptimisticSend::Queued,
            });
            desktop.model.draft_attachments = vec![big("fits.bin", 2), big("over.bin", 3)];
            desktop.model.connection_error = None;
            desktop.restore_pending_send(cx);
            let names = desktop
                .model
                .draft_attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["sent.bin", "fits.bin"]);
            assert!(
                desktop
                    .model
                    .connection_error
                    .as_deref()
                    .is_some_and(|error| error.contains("over.bin"))
            );
        });
    }

    #[test]
    fn file_sizes_read_the_way_people_write_them() {
        assert_eq!(file_size_label(812), "812 B");
        assert_eq!(file_size_label(14 * 1024 + 300), "14 KB");
        assert_eq!(file_size_label(3 * 1024 * 1024 + 200 * 1024), "3.2 MB");
        assert_eq!(file_size_label(12 * 1024 * 1024), "12 MB");
    }

    #[test]
    fn reading_attachments_refuses_folders_and_stops_once_the_draft_is_full() {
        let root = std::env::temp_dir().join(format!("xd-attach-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let report = root.join("Report.pdf");
        fs::write(&report, b"%PDF-1.7").unwrap();
        let read = read_attachment(&report, MAX_TOTAL_ATTACHMENT_BYTES).unwrap();
        assert_eq!(
            (read.name.as_str(), read.mime.as_str()),
            ("Report.pdf", "application/pdf")
        );
        assert!(read_attachment(&report, 7).unwrap_err().contains("20 MiB"));
        assert!(
            read_attachment(&root, MAX_TOTAL_ATTACHMENT_BYTES)
                .unwrap_err()
                .contains("is a folder")
        );
        let large = root.join("large.bin");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES as u64 + 1)
            .unwrap();
        assert!(
            read_attachment(&large, MAX_TOTAL_ATTACHMENT_BYTES)
                .unwrap_err()
                .contains("larger than 10 MiB")
        );
        #[cfg(unix)]
        {
            // A FIFO with no writer is refused at once rather than waited on.
            let fifo = root.join("fifo.txt");
            let path = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            assert!(
                read_attachment(&fifo, MAX_TOTAL_ATTACHMENT_BYTES)
                    .unwrap_err()
                    .contains("not a file")
            );
        }
        assert!(
            read_attachment(&root.join("missing"), MAX_TOTAL_ATTACHMENT_BYTES)
                .unwrap_err()
                .contains("could not be read")
        );

        let room = |slots, images, bytes| DraftRoom {
            slots,
            images,
            bytes,
        };
        // Two slots: the third file is never opened, and nothing after it.
        let chosen = vec![
            report.clone(),
            root.join("missing"),
            report.clone(),
            report.clone(),
            large,
        ];
        let read = read_attachments(chosen, room(2, 4, MAX_TOTAL_ATTACHMENT_BYTES));
        assert_eq!(read.len(), 4);
        assert!(read[0].is_ok() && read[1].is_err() && read[2].is_ok());
        assert!(read[3].as_ref().unwrap_err().contains("10 attachments"));
        // The byte budget shrinks with each file read.
        let small = root.join("small.txt");
        fs::write(&small, [1; 6]).unwrap();
        let read = read_attachments(vec![small.clone(), small], room(10, 4, 10));
        assert!(read[0].is_ok());
        assert!(read[1].as_ref().unwrap_err().contains("20 MiB"));
        // An image with no image slot left is refused without taking the slot
        // a file after it can still use.
        let shot = root.join("shot.png");
        fs::write(&shot, png(4, 4, |_, _| [0, 0, 0, 255])).unwrap();
        let read = read_attachments(vec![shot, report], room(1, 0, MAX_TOTAL_ATTACHMENT_BYTES));
        assert!(read[0].as_ref().unwrap_err().contains("4 images"));
        assert_eq!(read[1].as_ref().unwrap().name, "Report.pdf");
        fs::remove_dir_all(&root).unwrap();
    }

    #[gpui::test]
    fn a_removed_tile_forgets_its_thumbnail_and_does_not_open_the_viewer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 200), (200, 800)]);
        let generation = desktop.read_with(cx, |desktop, _| desktop.attachment_generation);

        click(cx, "remove-draft-image-0");

        cx.update(|_, cx| {
            let desktop = desktop.read(cx);
            assert!(desktop.image_preview.is_none());
            assert_eq!(desktop.model.draft_attachments.len(), 1);
            assert_eq!(desktop.model.draft_attachments[0].name, "image-1.png");
            assert!(desktop.attachments_dirty);
            assert_eq!(desktop.attachment_generation, generation + 1);
            assert_eq!(desktop.composer_thumbnails.len(), 1);
        });
        assert!(composer_is_focused(&desktop, cx));
    }

    #[gpui::test]
    fn the_viewer_opens_on_the_clicked_tile_and_steps_through_the_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 400), (400, 800)]);

        click(cx, "draft-image-1");
        cx.update(|window, cx| {
            let desktop = desktop.read(cx);
            assert!(desktop.minimal_popup_is_open());
            assert!(desktop.minimal_popup_focus.is_focused(window));
            let preview = desktop.image_preview.as_ref().expect("the viewer is open");
            assert_eq!(preview.index, 1);
            assert_eq!(preview.caption(), "image-1.png (2/2)");
        });

        click(cx, "image-preview-next");
        cx.update(|_, cx| {
            assert_eq!(desktop.read(cx).image_preview.as_ref().unwrap().index, 0);
        });
        cx.simulate_keystrokes("right");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(desktop.read(cx).image_preview.as_ref().unwrap().index, 1);
        });
        click(cx, "image-preview-previous");
        cx.update(|_, cx| {
            assert_eq!(desktop.read(cx).image_preview.as_ref().unwrap().index, 0);
        });

        let viewer = cx.debug_bounds("image-preview-viewer").unwrap();
        assert_eq!(viewer.size, size(px(800.0), px(400.0)));
        cx.update(|window, _| {
            let window = window.viewport_size();
            assert_eq!(viewer.center().x, window.width / 2.0);
        });
    }

    #[gpui::test]
    fn the_painted_viewer_is_where_the_zoom_arithmetic_thinks_it_is(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(4000, 3000)]);
        click(cx, "draft-image-0");

        for _ in 0..2 {
            let viewer = cx.debug_bounds("image-preview-viewer").unwrap();
            let layout = desktop.read_with(cx, |desktop, _| {
                desktop.image_preview.as_ref().unwrap().layout()
            });
            // Layout snaps to whole pixels, so the two agree to within one.
            for (painted, computed) in [
                (f32::from(viewer.left()), layout.x),
                (f32::from(viewer.top()), layout.y),
                (f32::from(viewer.size.width), layout.width),
                (f32::from(viewer.size.height), layout.height),
            ] {
                assert!(
                    (painted - computed).abs() <= 1.0,
                    "painted at {painted}, computed {computed}"
                );
            }
            click(cx, "image-preview-viewer");
        }
    }

    #[gpui::test]
    fn a_click_on_the_image_zooms_and_a_click_outside_closes(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 400)]);
        click(cx, "draft-image-0");
        assert!(cx.debug_bounds("image-preview-previous").is_none());

        click(cx, "image-preview-viewer");
        desktop.read_with(cx, |desktop, _| {
            assert_eq!(desktop.image_preview.as_ref().unwrap().zoom, CLICK_ZOOM);
        });
        click(cx, "image-preview-caption");
        desktop.read_with(cx, |desktop, _| {
            assert!(
                desktop.image_preview.is_some(),
                "the caption is not outside"
            );
        });

        cx.simulate_click(point(px(10.0), px(10.0)), gpui::Modifiers::default());
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| assert!(desktop.image_preview.is_none()));
        assert!(composer_is_focused(&desktop, cx));
    }

    #[gpui::test]
    fn the_close_button_and_escape_return_to_the_composer(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 400)]);

        click(cx, "draft-image-0");
        click(cx, "image-preview-close");
        desktop.read_with(cx, |desktop, _| assert!(desktop.image_preview.is_none()));
        assert!(composer_is_focused(&desktop, cx));

        click(cx, "draft-image-0");
        cx.dispatch_action(CloseSearch);
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| assert!(desktop.image_preview.is_none()));
        assert!(composer_is_focused(&desktop, cx));
    }

    #[gpui::test]
    fn dragging_a_zoomed_image_pans_it_without_toggling_the_zoom(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(4000, 3000)]);
        click(cx, "draft-image-0");
        click(cx, "image-preview-viewer");
        let scroll = desktop.read_with(cx, |desktop, _| {
            let preview = desktop.image_preview.as_ref().unwrap();
            assert_eq!(preview.zoom, CLICK_ZOOM);
            preview.scroll
        });

        let start = cx.debug_bounds("image-preview-viewer").unwrap().center();
        let end = start + point(px(-40.0), px(-20.0));
        let modifiers = gpui::Modifiers::default();
        cx.simulate_mouse_down(start, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(end, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(end, MouseButton::Left, modifiers);
        cx.run_until_parked();

        desktop.read_with(cx, |desktop, _| {
            let preview = desktop.image_preview.as_ref().unwrap();
            assert_eq!(
                preview.zoom, CLICK_ZOOM,
                "the drag's release is not a click"
            );
            assert!(preview.drag.is_none());
            assert_close(preview.scroll, (scroll.0 + 40.0, scroll.1 + 20.0));
        });
    }

    #[gpui::test]
    fn a_drag_released_outside_the_image_keeps_the_viewer_open(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(4000, 3000)]);
        click(cx, "draft-image-0");
        click(cx, "image-preview-viewer");

        let start = cx.debug_bounds("image-preview-viewer").unwrap().center();
        let outside = point(px(10.0), px(10.0));
        let modifiers = gpui::Modifiers::default();
        cx.simulate_mouse_down(start, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(outside, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(outside, MouseButton::Left, modifiers);
        cx.run_until_parked();

        desktop.read_with(cx, |desktop, _| {
            let preview = desktop
                .image_preview
                .as_ref()
                .expect("the viewer stays open");
            assert_eq!(preview.zoom, CLICK_ZOOM);
            assert!(preview.drag.is_none());
        });

        // The drag does not swallow the click that really is outside.
        cx.simulate_click(outside, modifiers);
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| assert!(desktop.image_preview.is_none()));
    }

    #[gpui::test]
    fn releasing_enter_or_space_does_not_close_the_viewer(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(4000, 3000)]);
        click(cx, "draft-image-0");

        for (key, zoom) in [("enter", CLICK_ZOOM), ("space", 1.0)] {
            cx.simulate_keystrokes(key);
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
            });
            cx.run_until_parked();
            desktop.read_with(cx, |desktop, _| {
                let preview = desktop
                    .image_preview
                    .as_ref()
                    .expect("the viewer stays open");
                assert_eq!(preview.zoom, zoom);
            });
        }
    }

    /// Press, let a synchronized draft put an image in front with no redraw, and
    /// release. A press draws a frame, so this is when a tile's position is stale.
    fn click_while_an_image_arrives(
        desktop: &Entity<XdDesktop>,
        cx: &mut gpui::VisualTestContext,
        selector: &'static str,
        arriving: &str,
    ) {
        let arrived = Attachment::from_png(arriving, png(300, 300, |_, _| [9, 9, 9, 255])).unwrap();
        let target = cx.debug_bounds(selector).unwrap().center();
        let modifiers = gpui::Modifiers::default();
        cx.simulate_mouse_down(target, MouseButton::Left, modifiers);
        desktop.update(cx, |desktop, _| {
            desktop.model.draft_attachments.insert(0, arrived);
        });
        cx.simulate_mouse_up(target, MouseButton::Left, modifiers);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_tile_opens_its_own_image_when_the_draft_changes_during_the_click(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 200), (200, 800)]);

        click_while_an_image_arrives(&desktop, cx, "draft-image-0", "arrived.png");

        desktop.read_with(cx, |desktop, _| {
            let preview = desktop.image_preview.as_ref().expect("the viewer is open");
            assert_eq!(preview.current().unwrap().name, "image-0.png");
        });
    }

    #[gpui::test]
    fn a_tile_removes_its_own_image_when_the_draft_changes_during_the_click(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 200), (200, 800)]);

        click_while_an_image_arrives(&desktop, cx, "remove-draft-image-0", "arrived.png");

        desktop.read_with(cx, |desktop, _| {
            let names = desktop
                .model
                .draft_attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["arrived.png", "image-1.png"]);
        });
    }

    #[gpui::test]
    fn an_image_too_large_to_decode_is_never_decoded(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[]);
        // Only the headers of an 8001x8000 PNG: past the limit, whatever the file size.
        let oversized = (0..90)
            .step_by(2)
            .map(|index| u8::from_str_radix(&"89504e470d0a1a0a0000000d4948445200001f4100001f400806000000e933c6ca000000004944415435af061e"[index..index + 2], 16).unwrap())
            .collect();
        desktop.update(cx, |desktop, cx| {
            desktop.model.draft_attachments =
                vec![Attachment::from_png("huge.png", oversized).unwrap()];
            cx.notify();
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let thumbnails = &desktop.read(cx).composer_thumbnails;
            assert_eq!(thumbnails.len(), 1);
            assert!(matches!(
                thumbnails.values().next(),
                Some(ThumbnailState::Failed)
            ));
        });
        click(cx, "draft-image-0");
        desktop.read_with(cx, |desktop, _| {
            let layout = desktop.image_preview.as_ref().unwrap().layout();
            assert_eq!(
                (layout.width, layout.height),
                (UNAVAILABLE_WIDTH, UNAVAILABLE_HEIGHT),
                "the viewer shows its panel rather than the image"
            );
        });
    }

    #[gpui::test]
    fn the_wheel_over_the_image_zooms_it(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = composer_with_images(cx, &[(4000, 3000)]);
        click(cx, "draft-image-0");

        let position = cx.debug_bounds("image-preview-viewer").unwrap().center();
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0.0, 3.0)),
            ..Default::default()
        });
        cx.run_until_parked();

        desktop.read_with(cx, |desktop, _| {
            let zoom = desktop.image_preview.as_ref().unwrap().zoom;
            let expected = (3.0 * WHEEL_LINE_HEIGHT * WHEEL_ZOOM_RATE).exp();
            assert!((zoom - expected).abs() < 0.001, "{zoom} is not {expected}");
        });
    }

    #[gpui::test]
    fn the_viewer_keeps_its_images_when_the_draft_changes_and_closes_with_its_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = composer_with_images(cx, &[(800, 400), (400, 800)]);
        click(cx, "draft-image-0");

        desktop.update(cx, |desktop, cx| {
            desktop.model.draft_attachments.clear();
            cx.notify();
        });
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| {
            assert_eq!(desktop.image_preview.as_ref().unwrap().images.len(), 2);
        });
        click(cx, "image-preview-viewer");
        desktop.read_with(cx, |desktop, _| {
            assert_eq!(desktop.image_preview.as_ref().unwrap().zoom, CLICK_ZOOM);
        });

        desktop.update(cx, |desktop, cx| {
            desktop.model.select_chat("review");
            cx.notify();
        });
        cx.run_until_parked();
        desktop.read_with(cx, |desktop, _| {
            assert!(desktop.image_preview.is_none());
            assert!(!desktop.minimal_popup_is_open());
        });
    }

    #[test]
    fn the_chevron_left_icon_is_embedded() {
        let bytes = EmbeddedIcons
            .load(CHEVRON_LEFT_ICON)
            .expect("embedded icons load")
            .expect("the icon is embedded");
        assert!(String::from_utf8_lossy(&bytes).contains("<svg"));
    }

    #[test]
    fn an_image_with_no_readable_size_gets_a_panel_and_does_not_zoom() {
        let images = vec![PreviewImage {
            name: "broken.png".into(),
            image: Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())),
            size: None,
        }];
        let mut preview = ImagePreview::new("chat".into(), images, 0, (1400.0, 1000.0));
        let layout = preview.layout();
        assert_eq!(
            (layout.width, layout.height),
            (UNAVAILABLE_WIDTH, UNAVAILABLE_HEIGHT)
        );
        preview.toggle_zoom(None);
        assert_eq!(preview.zoom, 1.0);
    }
}
