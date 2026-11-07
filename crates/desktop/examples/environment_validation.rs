use std::{fs, path::PathBuf};

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowOptions, application, assets, div, init, open_window, px,
    rgb,
};
use serde::{Deserialize, Serialize};

const WINDOW_TITLE: &str = "GPUI Environment Validation";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SavedFixture {
    text: String,
    checked: bool,
    action_count: u32,
}

fn fixture_path() -> PathBuf {
    let name = if cfg!(test) {
        "test-fixture.json"
    } else {
        "fixture.json"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gui-validation")
        .join(name)
}

struct ValidationView {
    input: Entity<InputState>,
    checked: bool,
    action_count: u32,
    status: String,
}

impl ValidationView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the Unicode test text"));
        Self {
            input,
            checked: false,
            action_count: 0,
            status: "Ready".into(),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let fixture = SavedFixture {
            text: self.input.read(cx).value().to_string(),
            checked: self.checked,
            action_count: self.action_count,
        };
        let path = fixture_path();
        let result = (|| -> std::io::Result<()> {
            fs::create_dir_all(path.parent().expect("fixture has parent"))?;
            let json = serde_json::to_vec_pretty(&fixture)?;
            fs::write(&path, json)
        })();
        self.status = match result {
            Ok(()) => format!("Saved: {}", path.display()),
            Err(error) => format!("Save failed: {error}"),
        };
        cx.notify();
    }
}

impl Render for ValidationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_4()
            .p_6()
            .bg(rgb(0xf8fafc))
            .text_color(rgb(0x111827))
            .child(div().text_xl().child(WINDOW_TITLE))
            .child(div().child("Native Wayland + GPUI Kit control check"))
            .child(Input::new(&self.input).id("validation-input").w_full())
            .child(
                Checkbox::new("validation-checkbox")
                    .label("Enable fixture flag")
                    .checked(self.checked)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.checked = *checked;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        Button::new("action-button")
                            .primary()
                            .label("Count action")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_count += 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-button")
                            .label("Save fixture")
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
            .child(div().child(format!("Action count: {}", self.action_count)))
            .child(div().child(self.status.clone()))
            .child(div().child("Scroll this list:"))
            .child(
                div()
                    .id("validation-scroll-area")
                    .flex()
                    .flex_col()
                    .w_full()
                    .h(px(170.))
                    .gap_1()
                    .p_2()
                    .bg(rgb(0xe2e8f0))
                    .children((1..=30).map(|number| {
                        div()
                            .id(("validation-row", number as u32))
                            .child(format!("Scrollable row {number:02}"))
                    }))
                    .overflow_y_scrollbar(),
            )
    }
}

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        open_window(
            WindowOptions {
                app_id: Some("dev.objekte.gpui-environment-validation".into()),
                ..WindowOptions::default()
            },
            cx,
            |window, cx| {
                window.set_window_title(WINDOW_TITLE);
                cx.new(|cx| ValidationView::new(window, cx))
            },
        )
        .expect("failed to open validation window");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt as _;

    #[gpui_kit::test]
    fn saves_unicode_fixture_through_controls(cx: &mut TestAppContext) {
        cx.update(init);
        let (handle, view) = cx.update(|cx| {
            open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| ValidationView::new(window, cx))
            })
            .expect("open test window")
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("validation-input", cx);
            window.input("Objekte ÄÖÜ äöü ß 123", cx);
            window.click("validation-checkbox", cx);
            window.click("action-button", cx);
            window.click("save-button", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert_eq!(view.read(cx).action_count, 1);
            assert!(view.read(cx).checked);
        });
        let fixture: SavedFixture =
            serde_json::from_slice(&fs::read(fixture_path()).unwrap()).unwrap();
        assert_eq!(
            fixture,
            SavedFixture {
                text: "Objekte ÄÖÜ äöü ß 123".into(),
                checked: true,
                action_count: 1,
            }
        );
    }
}
