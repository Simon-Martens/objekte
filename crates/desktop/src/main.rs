use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    WindowOptions, application, assets, div, init, open_window, rgb,
};


/// Temporary window while the repository and interaction modules are integrated.
struct BootstrapView;


impl Render for BootstrapView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_6()
            .bg(rgb(0x101820))
            .text_color(rgb(0xe7edf2))
            .child(div().text_xl().child("Objekte"))
            .child(div().child("Desktop workspace is loading"))
    }
}


fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        open_window(
            WindowOptions {
                app_id: Some("dev.objekte.desktop".into()),
                ..WindowOptions::default()
            },
            cx,
            |window, cx| {
                window.set_window_title("Objekte");
                cx.new(|_| BootstrapView)
            },
        )
        .expect("failed to open Objekte window");
    });
}
