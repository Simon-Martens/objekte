//! Native Objekte application entry point.

use std::time::Duration;

use gpui_kit::{AppContext as _, WindowOptions, application, assets, init, open_window};
use objekte_desktop::controller::runtime::DesktopController;
use objekte_desktop::ui::model::{UiAction, UiUpdate};
use objekte_desktop::ui::typography::register_fonts;
use objekte_desktop::ui::{DesktopView, init_keybindings};

fn main() {
    application().with_assets(assets::AllAssets).run(|cx| {
        init(cx);
        init_keybindings(cx);
        register_fonts(cx).expect("bundled desktop fonts must load before the first window");
        let (mut controller, actions) = DesktopController::new();
        let close_actions = actions.clone();
        let (window_handle, view) = open_window(
            WindowOptions {
                app_id: Some("dev.objekte.desktop".into()),
                ..WindowOptions::default()
            },
            cx,
            |window, cx| {
                window.set_window_title("Objekte");
                cx.new(|cx| DesktopView::new(window, cx, actions))
            },
        )
        .expect("failed to open Objekte window");
        window_handle
            .update(cx, |_, window, app| {
                window.on_window_should_close(app, move |_, _| {
                    let _ = close_actions.send(UiAction::RequestClose);
                    false
                });
            })
            .expect("failed to install index shutdown hook");
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            loop {
                executor.timer(Duration::from_millis(16)).await;
                let updates = cx.update(|app| controller.tick(app));
                if updates.is_empty() {
                    continue;
                }
                let close_ready = updates
                    .iter()
                    .any(|update| matches!(update, UiUpdate::CloseReady));
                if window_handle
                    .update(cx, |_, window, app| {
                        view.update(app, |view, cx| {
                            for update in updates {
                                view.apply(update, window, cx);
                            }
                        });
                    })
                    .is_err()
                {
                    break;
                }
                if close_ready {
                    let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                    break;
                }
            }
        })
        .detach();
    });
}
