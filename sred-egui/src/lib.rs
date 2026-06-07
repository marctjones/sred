//! egui host adapter for the sred editor component.
//!
//! Proves `sred-core` is genuinely host-agnostic: this crate depends only on the
//! public `Editor` API. It blits the editor's RGBA frame into an egui texture,
//! forwards keyboard / IME / pointer / clipboard events, and feeds egui's
//! accessibility (which in turn drives AccessKit) from the editor's a11y snapshot.
//!
//! ```ignore
//! let mut widget = sred_egui::SredWidget::from_source(&body, Format::Markdown);
//! egui::CentralPanel::default().show(ctx, |ui| { widget.ui(ui); });
//! // persist with widget.editor().text() — byte-lossless.
//! ```

use sred_core::editor::{Command, Motion};
use sred_core::{Editor, Format};

/// An egui widget wrapping a sred [`Editor`].
pub struct SredWidget {
    editor: Editor,
    tex: Option<egui::TextureHandle>,
    /// Per-frame textures for rendered math fragments (kept alive while painting).
    frag_texes: Vec<egui::TextureHandle>,
}

impl SredWidget {
    pub fn new(format: Format) -> Self {
        SredWidget {
            editor: Editor::new(format),
            tex: None,
            frag_texes: Vec::new(),
        }
    }

    pub fn from_source(src: &str, format: Format) -> Self {
        SredWidget {
            editor: Editor::from_source(src, format),
            tex: None,
            frag_texes: Vec::new(),
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Handle one egui event that needs no pointer geometry (text, key, IME,
    /// paste). Returns whether it was consumed. Pointer/clipboard-output events
    /// are handled in [`ui`](Self::ui). Factored out so input mapping is testable
    /// without an egui context.
    pub fn apply_event(&mut self, ev: &egui::Event) -> bool {
        use egui::{Event, ImeEvent};
        match ev {
            Event::Text(t) if !t.is_empty() => {
                self.editor.apply(Command::Insert(t.clone()));
                true
            }
            Event::Paste(t) => {
                self.editor.paste(t);
                true
            }
            Event::Ime(ime) => {
                match ime {
                    ImeEvent::Preedit(s) if s.is_empty() => self.editor.clear_preedit(),
                    ImeEvent::Preedit(s) => self.editor.set_preedit(s, s.chars().count()),
                    ImeEvent::Commit(s) => self.editor.commit_preedit(s),
                    ImeEvent::Enabled | ImeEvent::Disabled => self.editor.clear_preedit(),
                }
                true
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => self.apply_key(*key, *modifiers),
            _ => false,
        }
    }

    fn apply_key(&mut self, key: egui::Key, mods: egui::Modifiers) -> bool {
        use egui::Key;
        let shift = mods.shift;
        let word = mods.ctrl || mods.alt; // word granularity
        let doc = mods.command; // ctrl (or ⌘ on macOS)
                                // Build a Move/Select command for a horizontal/doc motion.
        let go = |this: &mut Self, m: Motion| {
            this.editor.apply(if shift {
                Command::Select(m)
            } else {
                Command::Move(m)
            });
        };
        match key {
            Key::ArrowLeft => go(self, if word { Motion::WordLeft } else { Motion::Left }),
            Key::ArrowRight => go(
                self,
                if word {
                    Motion::WordRight
                } else {
                    Motion::Right
                },
            ),
            Key::ArrowUp => self.editor.move_vertical(false),
            Key::ArrowDown => self.editor.move_vertical(true),
            Key::Home => go(
                self,
                if doc {
                    Motion::DocStart
                } else {
                    Motion::LineStart
                },
            ),
            Key::End => go(self, if doc { Motion::DocEnd } else { Motion::LineEnd }),
            Key::PageUp => self.editor.page(false),
            Key::PageDown => self.editor.page(true),
            Key::Backspace => self.editor.apply(if word {
                Command::DeleteWordBackward
            } else {
                Command::DeleteBackward
            }),
            Key::Delete => self.editor.apply(if word {
                Command::DeleteWordForward
            } else {
                Command::DeleteForward
            }),
            Key::Enter => self.editor.apply(Command::Insert("\n".into())),
            Key::Tab => self.editor.apply(if shift {
                Command::Outdent
            } else {
                Command::Indent
            }),
            Key::A if doc => self.editor.apply(Command::SelectAll),
            Key::Z if doc => self
                .editor
                .apply(if shift { Command::Redo } else { Command::Undo }),
            Key::Y if doc => self.editor.apply(Command::Redo),
            _ => return false,
        }
        true
    }

    /// Render + drive the editor for one egui frame. Returns the widget response.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let avail = ui.available_size();
        self.editor
            .set_viewport(avail.x.max(1.0) as u32, avail.y.max(1.0));

        let (rect, response) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
        let focused = response.has_focus();

        // Keyboard / IME / clipboard events (only while focused).
        if focused {
            let events = ui.input(|i| i.events.clone());
            for ev in &events {
                match ev {
                    egui::Event::Copy => {
                        let t = self.editor.copy();
                        if !t.is_empty() {
                            ui.ctx().copy_text(t);
                        }
                    }
                    egui::Event::Cut => {
                        let t = self.editor.cut();
                        if !t.is_empty() {
                            ui.ctx().copy_text(t);
                        }
                    }
                    other => {
                        self.apply_event(other);
                    }
                }
            }
            let dy = ui.input(|i| i.raw_scroll_delta.y);
            if dy != 0.0 {
                self.editor.scroll_by(-dy);
            }
        }

        // Pointer: document-space coords = viewport-local + scroll.
        if let Some(pos) = response.interact_pointer_pos() {
            let local = pos - rect.min;
            let (x, y) = (local.x, self.editor.scroll_y() + local.y);
            if response.triple_clicked() {
                response.request_focus();
                self.editor.triple_click(x, y);
            } else if response.double_clicked() {
                response.request_focus();
                self.editor.double_click(x, y);
            } else if response.dragged() {
                self.editor.drag(x, y);
            } else if response.clicked() {
                response.request_focus();
                if ui.input(|i| i.modifiers.alt) {
                    self.editor.add_caret_at(x, y); // Alt+click → multi-cursor
                } else {
                    self.editor.click(x, y);
                }
            }
        }

        // Render the viewport slice and blit it.
        let out = self.editor.render_view(focused);
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [out.frame.width as usize, out.frame.height as usize],
            &out.frame.rgba,
        );
        let opts = egui::TextureOptions::NEAREST;
        if let Some(h) = &mut self.tex {
            h.set(img, opts);
        } else {
            self.tex = Some(ui.ctx().load_texture("sred", img, opts));
        }
        let tex_id = self.tex.as_ref().unwrap().id();
        let painter = ui.painter_at(rect);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);

        // Overlay rendered math fragments over their source spans (#15).
        self.frag_texes.clear();
        if self.editor.has_fragment_renderer() {
            let frags = self.editor.math_fragments();
            for frag in &frags {
                if let Some(img) = self.editor.render_fragment(frag) {
                    let cimg = egui::ColorImage::from_rgba_unmultiplied(
                        [img.width as usize, img.height as usize],
                        &img.rgba,
                    );
                    let tex =
                        ui.ctx()
                            .load_texture("sred-frag", cimg, egui::TextureOptions::LINEAR);
                    let size = egui::vec2(img.width as f32, img.height as f32);
                    for r in self.editor.rect_for_range(frag.start, frag.end) {
                        let at = egui::Rect::from_min_size(rect.min + egui::vec2(r.x, r.y), size);
                        painter.image(tex.id(), at, uv, egui::Color32::WHITE);
                    }
                    self.frag_texes.push(tex);
                }
            }
        }

        // Carets: primary + any secondary multi-cursors (#17).
        if focused {
            for c in &out.carets {
                let caret_rect = egui::Rect::from_min_size(
                    rect.min + egui::vec2(c.x, c.y),
                    egui::vec2(1.5, c.h),
                );
                painter.rect_filled(caret_rect, 0.0, egui::Color32::from_rgb(33, 33, 33));
            }
        }

        // Feed egui accessibility (drives AccessKit) from the editor snapshot.
        let snap = self.editor.a11y();
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::new(egui::WidgetType::TextEdit);
            info.current_text_value = Some(snap.value.clone());
            info
        });

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, ImeEvent, Key, Modifiers};

    fn key(k: Key, mods: Modifiers) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: mods,
        }
    }

    #[test]
    fn text_event_inserts() {
        let mut w = SredWidget::new(Format::Markdown);
        assert!(w.apply_event(&Event::Text("hi".into())));
        assert_eq!(w.editor().text(), "hi");
    }

    #[test]
    fn ime_preedit_then_commit() {
        let mut w = SredWidget::from_source("a", Format::Markdown);
        w.apply_event(&Event::Text("b".into())); // "ab", caret at end
        w.apply_event(&Event::Ime(ImeEvent::Preedit("ni".into())));
        assert_eq!(w.editor().text(), "ab", "preedit is not in the buffer");
        w.apply_event(&Event::Ime(ImeEvent::Commit("\u{4f60}".into())));
        assert_eq!(w.editor().text(), "ab\u{4f60}");
    }

    #[test]
    fn word_motion_and_delete_via_keys() {
        let mut w = SredWidget::from_source("alpha beta", Format::Markdown);
        w.editor_mut().core_mut().set_cursor(0);
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        w.apply_event(&key(Key::ArrowRight, ctrl)); // word-right past "alpha"
        assert_eq!(w.editor_mut().core_mut().cursor(), 5);
        w.apply_event(&key(Key::Delete, ctrl)); // delete " beta" word
        assert_eq!(w.editor().text(), "alpha");
    }

    #[test]
    fn select_all_and_undo_via_command_modifier() {
        let mut w = SredWidget::from_source("hello", Format::Markdown);
        let cmd = Modifiers {
            command: true,
            ..Default::default()
        };
        w.apply_event(&key(Key::A, cmd));
        assert_eq!(w.editor().selected_text(), "hello");
        w.apply_event(&Event::Text("x".into())); // replaces selection
        assert_eq!(w.editor().text(), "x");
        w.apply_event(&key(Key::Z, cmd)); // undo
        assert_eq!(w.editor().text(), "hello");
    }
}
