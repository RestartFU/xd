use gpui::{
    App, Application, Bounds, Context, ListAlignment, ListState, Render, Window, WindowBounds,
    WindowOptions, div, list, prelude::*, px, rgb, size,
};
use xd_desktop::model::{AppModel, Message};

const BG: u32 = 0x111318;
const SURFACE: u32 = 0x191c22;
const SURFACE_HIGH: u32 = 0x232730;
const BORDER: u32 = 0x303641;
const TEXT: u32 = 0xe8eaf0;
const MUTED: u32 = 0x969daa;
const ACCENT: u32 = 0x6b8cff;

struct XdDesktop {
    model: AppModel,
    transcript: ListState,
}

impl XdDesktop {
    fn demo() -> Self {
        let model = AppModel::demo();
        let transcript =
            ListState::new(model.messages.len(), ListAlignment::Bottom, px(700.0)).measure_all();
        Self { model, transcript }
    }

    fn message_row(message: &Message) -> impl IntoElement {
        let is_user = message.role == "user";
        let is_tool = message.role == "tool";
        let label = message
            .label
            .clone()
            .unwrap_or_else(|| match message.role.as_str() {
                "user" => "You".into(),
                "assistant" => "Assistant".into(),
                "tool" => "Activity".into(),
                role => role.to_owned(),
            });

        div().w_full().px_6().py_2().child(
            div()
                .w_full()
                .max_w(px(920.0))
                .mx_auto()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(rgb(if is_user { 0x3c4b78 } else { BORDER }))
                .bg(rgb(if is_user {
                    0x202944
                } else if is_tool {
                    0x171a20
                } else {
                    SURFACE
                }))
                .text_color(rgb(TEXT))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(if is_user { 0xaec0ff } else { MUTED }))
                        .mb_2()
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .line_height(px(21.0))
                        .child(message.content.clone()),
                ),
        )
    }
}

impl Render for XdDesktop {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let messages = self.model.messages.clone();
        let queue_count = self.model.queue.len();
        let working = self.model.working;

        let sidebar = div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(58.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_lg().text_color(rgb(TEXT)).child("xd"))
                    .child(div().text_xs().text_color(rgb(0x92d5a5)).child("connected")),
            )
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("WORKSPACES"),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child("▾  xd"),
            )
            .children(self.model.chats.iter().map(|chat| {
                let selected = self.model.selected_chat.as_deref() == Some(chat.id.as_str());
                div()
                    .mx_2()
                    .mb_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(if selected { SURFACE_HIGH } else { SURFACE }))
                    .text_color(rgb(if selected { TEXT } else { MUTED }))
                    .text_sm()
                    .child(if chat.working {
                        format!("●  {}", chat.title)
                    } else {
                        format!("   {}", chat.title)
                    })
            }));

        let header = div()
            .h(px(58.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px_5()
            .bg(rgb(SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child("Rewrite desktop with GPUI"),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child("xd · codex")),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(if working { 0x26354d } else { SURFACE_HIGH }))
                    .text_xs()
                    .text_color(rgb(if working { 0xaec0ff } else { MUTED }))
                    .child(if working { "Working…" } else { "Ready" }),
            );

        let transcript = list(self.transcript.clone(), move |index, _window, _cx| {
            Self::message_row(&messages[index]).into_any_element()
        })
        .size_full();

        let composer = div()
            .flex_shrink_0()
            .px_5()
            .pt_2()
            .pb_4()
            .bg(rgb(BG))
            .when(queue_count > 0, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x24212f))
                        .text_xs()
                        .text_color(rgb(0xc8b6e8))
                        .child(format!("{queue_count} queued message")),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .mx_auto()
                    .min_h(px(74.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("Message xd…"),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(ACCENT))
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(if working { "Queue" } else { "Send" }),
                    ),
            );

        div()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .font_family("Inter")
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(composer),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| XdDesktop::demo()),
        )
        .expect("open xd GPUI window");
        cx.activate(true);
    });
}
