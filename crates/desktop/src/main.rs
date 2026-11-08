//! Native Objekte application entry point.

use std::time::Duration;

use gpui_kit::{AppContext as _, WindowOptions, application, assets, init, open_window};
use objekte_desktop::controller::runtime::DesktopController;
use objekte_desktop::ui::DesktopView;

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        let (mut controller, actions) = DesktopController::new();
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
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            loop {
                executor.timer(Duration::from_millis(16)).await;
                let updates = cx.update(|app| controller.tick(app));
                if updates.is_empty() {
                    continue;
                }
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
            }
        })
        .detach();
    });
}
