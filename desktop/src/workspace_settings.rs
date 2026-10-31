use super::*;

const SETTINGS_SECTIONS: [(&str, &str); 5] = [
    ("Appearance", "A familiar workspace, in your own light."),
    ("General", "Preferences for your daily work."),
    ("Agents", "Each chat can use a different agent."),
    ("Devices", "Your sessions, wherever you are."),
    ("Shortcuts", "Keep your hands on the keyboard."),
];

fn settings_heading(title: &'static str, colors: ThemeColors) -> gpui::Div {
    div()
        .mt(px(26.0))
        .mb(px(10.0))
        .text_size(px(14.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors.text))
        .child(title)
}

fn settings_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .min_h(px(72.0))
        .py(px(16.0))
        .flex()
        .items_center()
        .gap_4()
        .border_b_1()
        .border_color(rgb(colors.border))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(colors.text))
                        .child(title),
                )
                .when(!description.is_empty(), |body| {
                    body.child(
                        div()
                            .mt(px(5.0))
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(colors.muted))
                            .child(description),
                    )
                }),
        )
}

impl XdDesktop {
    pub(super) fn render_workspace_settings(
        &mut self,
        colors: ThemeColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let width = (f32::from(window.viewport_size().width) - 48.0).clamp(280.0, 900.0);
        let height = (f32::from(window.viewport_size().height) - 64.0).clamp(240.0, 760.0);
        let compact = width < 640.0;
        let section = self.settings_section.min(SETTINGS_SECTIONS.len() - 1);
        let minimal_popup_focus = self.minimal_popup_focus.clone();
        let (title, description) = SETTINGS_SECTIONS[section];

        let navigation = div()
            .id("workspace-settings-navigation")
            .flex_none()
            .p(px(10.0))
            .bg(rgba(colors.sidebar_glass))
            .flex()
            .when(!compact, |nav| {
                nav.w(px(176.0))
                    .h_full()
                    .flex_col()
                    .gap(px(4.0))
                    .border_r_1()
            })
            .when(compact, |nav| {
                nav.w_full()
                    .h(px(56.0))
                    .items_center()
                    .gap(px(8.0))
                    .overflow_x_scroll()
                    .border_b_1()
            })
            .border_color(rgb(colors.border))
            .children(
                SETTINGS_SECTIONS
                    .iter()
                    .enumerate()
                    .map(|(index, (name, _))| {
                        let selected = section == index;
                        div()
                            .id(("workspace-settings-section", index))
                            .h(px(38.0))
                            .px(px(12.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .rounded(px(8.0))
                            .text_size(px(13.0))
                            .font_weight(if selected {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(rgb(if selected { colors.text } else { colors.muted }))
                            .when(selected, |tab| tab.bg(rgb(colors.selected_surface)))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings_section = index;
                                cx.notify();
                            }))
                            .child(*name)
                    }),
            );

        let mut content = div()
            .id(("workspace-settings-content", section))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .p(px(if compact { 20.0 } else { 32.0 }))
            .bg(rgb(colors.background))
            .text_color(rgb(colors.text))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(rgb(colors.muted))
                    .child(description),
            );

        match section {
            0 => {
                content = content.child(settings_heading("Theme", colors)).child(
                    div()
                        .flex()
                        .gap_4()
                        .children(ThemePreset::ALL.into_iter().enumerate().map(
                            |(index, theme)| {
                                let swatch = theme.colors();
                                let selected = self.settings.theme == theme;
                                div()
                                    .id(("minimal-theme-choice", index))
                                    .flex_1()
                                    .min_w_0()
                                    .rounded(px(12.0))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(rgb(if selected {
                                        colors.selected_border
                                    } else {
                                        colors.border
                                    }))
                                    .bg(rgb(colors.surface))
                                    .cursor_pointer()
                                    .hover(|style| style.border_color(rgb(colors.selected_border)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_minimal_theme(theme, cx);
                                        this.minimal_theme_open = true;
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .h(px(112.0))
                                            .p_3()
                                            .flex()
                                            .gap(px(10.0))
                                            .bg(rgb(swatch.background))
                                            .child(
                                                div()
                                                    .w(px(28.0))
                                                    .h_full()
                                                    .rounded(px(5.0))
                                                    .bg(rgb(swatch.sidebar)),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .pt_3()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .w(px(72.0))
                                                            .h(px(5.0))
                                                            .rounded_full()
                                                            .bg(rgb(swatch.muted)),
                                                    )
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .h(px(4.0))
                                                            .rounded_full()
                                                            .bg(rgb(swatch.border)),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(56.0))
                                                            .h(px(4.0))
                                                            .rounded_full()
                                                            .bg(rgb(swatch.border)),
                                                    )
                                                    .child(
                                                        div()
                                                            .mt_2()
                                                            .w_full()
                                                            .h(px(24.0))
                                                            .rounded(px(7.0))
                                                            .border_1()
                                                            .border_color(rgb(swatch.border))
                                                            .bg(rgb(swatch.surface)),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .h(px(44.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .text_size(px(13.0))
                                            .child(theme.label())
                                            .when(selected, |label| {
                                                label.child(
                                                    div()
                                                        .size(px(7.0))
                                                        .rounded_full()
                                                        .bg(rgb(colors.accent)),
                                                )
                                            }),
                                    )
                            },
                        )),
                );
            }
            1 => {
                content = content
                    .child(settings_heading("Sessions", colors))
                    .child(
                        settings_row(
                            "workspace-settings-notifications",
                            "Session notifications",
                            "Notify when a background chat finishes.",
                            colors,
                        )
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.notifications = !this.settings.notifications;
                            if let Err(error) = this.settings.save() {
                                this.model.connection_error = Some(error);
                            }
                            cx.notify();
                        }))
                        .child(settings_switch(colors, self.settings.notifications)),
                    )
                    .child(
                        settings_row(
                            "workspace-settings-speech",
                            "Read replies aloud",
                            "Speak assistant output when speech tags are available.",
                            colors,
                        )
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.speech = !this.settings.speech;
                            if !this.settings.speech {
                                this.pending_speech = None;
                                this.speech_output.stop();
                            }
                            if let Err(error) = this.settings.save() {
                                this.model.connection_error = Some(error);
                            }
                            cx.notify();
                        }))
                        .child(settings_switch(colors, self.settings.speech)),
                    )
                    .child(settings_heading("Updates", colors));
                let status = self
                    .self_update_panel
                    .as_ref()
                    .map(|panel| {
                        if let Some(error) = &panel.error {
                            return error.clone();
                        }
                        match panel.status.as_ref() {
                            Some(status) if status.state == "installed" => {
                                "Installed. Restart xd to finish updating.".into()
                            }
                            Some(status) if status.state == "installing" => {
                                "Installing update…".into()
                            }
                            Some(status) if status.state == "checking" => {
                                "Checking for updates…".into()
                            }
                            Some(status) if !status.supported => {
                                "This installation requires a manual update.".into()
                            }
                            Some(status) if status.available => format!(
                                "Update available: {}",
                                status.latest.as_deref().unwrap_or("new version")
                            ),
                            Some(status) if status.latest.is_none() => {
                                format!("xd {} · Check for updates", status.version)
                            }
                            Some(status) => format!("xd {} · Up to date", status.version),
                            None => "Connecting to the local update service…".into(),
                        }
                    })
                    .unwrap_or_else(|| "Check for an update to xd.".into());
                content = content.child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(19.0))
                        .text_color(rgb(colors.muted))
                        .child(status),
                );
                let busy = self
                    .self_update_panel
                    .as_ref()
                    .is_some_and(|panel| panel.busy);
                let installed = self.self_update_panel.as_ref().is_some_and(|panel| {
                    panel
                        .status
                        .as_ref()
                        .is_some_and(|status| status.state == "installed")
                });
                let available = self.self_update_panel.as_ref().is_some_and(|panel| {
                    panel
                        .status
                        .as_ref()
                        .is_some_and(|status| status.supported && status.available)
                });
                content = content.child(
                    div()
                        .mt_3()
                        .flex()
                        .gap_2()
                        .when(!busy && !installed, |actions| {
                            actions.child(
                                ui_button("check-app-update", ButtonKind::Secondary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request_app_update("check", cx)
                                    }))
                                    .child("Check for updates"),
                            )
                        })
                        .when(!busy && !installed && available, |actions| {
                            actions.child(
                                ui_button("install-app-update", ButtonKind::Primary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request_app_update("install", cx)
                                    }))
                                    .child("Install update"),
                            )
                        })
                        .when(!busy && installed, |actions| {
                            actions.child(
                                ui_button("restart-app-update", ButtonKind::Primary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.flush_draft_before_navigation() {
                                            this.request_app_update("restart", cx);
                                        }
                                    }))
                                    .child("Restart to update"),
                            )
                        }),
                );
                let running = self.source_build_panel.running;
                let can_build = source_build::supported()
                    && self.source_build_panel.target.is_some()
                    && !running;
                content = content
                    .child(settings_heading("Build from source", colors))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(19.0))
                            .text_color(rgb(colors.muted))
                            .child(if source_build::supported() {
                                "Build and install a branch, pull request, or commit."
                            } else {
                                "Source builds are available in an installed nightly."
                            }),
                    )
                    .child(
                        div()
                            .mt_3()
                            .h(px(38.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(rgb(colors.selected_border))
                            .bg(rgb(colors.surface))
                            .child(self.source_build_input.clone()),
                    )
                    .child(
                        div()
                            .mt_3()
                            .flex()
                            .gap_2()
                            .child(
                                ui_button(
                                    "workspace-settings-source-build",
                                    ButtonKind::Secondary,
                                    colors,
                                )
                                .when(!can_build, |button| button.opacity(0.5))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_build {
                                        this.start_source_build(cx);
                                    }
                                }))
                                .child(if running {
                                    "Building…"
                                } else {
                                    "Build and install"
                                }),
                            )
                            .when(running, |actions| {
                                actions.child(
                                    ui_button(
                                        "workspace-settings-source-stop",
                                        ButtonKind::Ghost,
                                        colors,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(run) = &this.source_build_run {
                                            run.cancel();
                                        }
                                        this.source_build_panel.stopping = true;
                                        cx.notify();
                                    }))
                                    .child(
                                        if self.source_build_panel.stopping {
                                            "Stopping…"
                                        } else {
                                            "Stop build"
                                        },
                                    ),
                                )
                            }),
                    )
                    .when_some(self.source_build_panel.message.clone(), |body, message| {
                        body.child(
                            div()
                                .mt_3()
                                .text_size(px(12.0))
                                .line_height(px(19.0))
                                .text_color(rgb(colors.muted))
                                .child(message),
                        )
                    })
                    .when(!self.source_build_panel.output.is_empty(), |body| {
                        body.child(
                            div()
                                .id("workspace-settings-build-output")
                                .mt_3()
                                .max_h(px(180.0))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded(px(8.0))
                                .bg(rgb(colors.surface))
                                .font_family(MONO)
                                .text_size(px(11.5))
                                .line_height(px(17.0))
                                .child(
                                    self.source_build_panel
                                        .output
                                        .iter()
                                        .cloned()
                                        .collect::<String>(),
                                ),
                        )
                    });
            }
            2 => {
                content = content
                    .child(settings_heading("Accounts and command-line tools", colors))
                    .child(
                        settings_row("workspace-settings-accounts", "Assistant accounts", "Manage sign-in and the installed assistant CLIs.", colors)
                            .child(ui_button("assistant-accounts", ButtonKind::Secondary, colors)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.minimal_theme_open = false;
                                    this.auth_open = true;
                                    if let Some(host) = this.active_host() { let _ = host.agent_auth(); let _ = host.agent_clis(); }
                                    this.focus_minimal_popup(this.minimal_popup_focus.clone(), window, cx);
                                    cx.notify();
                                }))
                                .child("Manage")),
                    )
                    .child(settings_heading("Permissions", colors))
                    .child(
                        settings_row("minimal-all-permissions", "All permissions", "New agent tabs skip approvals and sandboxing when the CLI supports it.", colors)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_minimal_all_permissions(cx)))
                            .child(settings_switch(colors, self.settings.allow_all_permissions)),
                    );
            }
            3 => {
                content = content
                    .child(settings_heading("Workspace connection", colors))
                    .child(
                        settings_row(
                            "workspace-settings-connection",
                            "This device",
                            "Sessions keep running when the window closes.",
                            colors,
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(if self.model.connected {
                                    colors.working
                                } else {
                                    colors.muted
                                }))
                                .child(if self.active_endpoint == ChatEndpoint::Local {
                                    "Local"
                                } else {
                                    "Remote"
                                }),
                        ),
                    );
                if self.active_endpoint == ChatEndpoint::Local {
                    content = content
                        .child(
                            settings_row(
                                "workspace-settings-pair",
                                "Pair another device",
                                "Connect another desktop or the Android app.",
                                colors,
                            )
                            .child(
                                ui_button("pair-device", ButtonKind::Secondary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.minimal_theme_open = false;
                                        this.share_panel = Some(SharePanel {
                                            loading: true,
                                            ..Default::default()
                                        });
                                        if let Some(host) = &this.host {
                                            let _ = host.peer_pairing();
                                        }
                                        cx.notify();
                                    }))
                                    .child("Pair a device"),
                            ),
                        )
                        .child(
                            settings_row(
                                "workspace-settings-devices",
                                "Paired devices",
                                "View devices that can connect to this host.",
                                colors,
                            )
                            .child(
                                ui_button("paired-devices", ButtonKind::Secondary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.minimal_theme_open = false;
                                        this.devices_panel = Some(DevicesPanel {
                                            loading: true,
                                            ..Default::default()
                                        });
                                        this.request_devices();
                                        cx.notify();
                                    }))
                                    .child("Manage"),
                            ),
                        );
                }
                content = content
                    .child(settings_heading("Remote host", colors))
                    .child(
                        settings_row(
                            "workspace-settings-remote",
                            "Connect to another host",
                            "Pair with a host using the code it provides.",
                            colors,
                        )
                        .child(
                            ui_button("workspace-settings-connect", ButtonKind::Secondary, colors)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.minimal_theme_open = false;
                                    this.open_remote(window, cx);
                                }))
                                .child("Connect"),
                        ),
                    )
                    .when(self.active_endpoint == ChatEndpoint::Remote, |body| {
                        body.child(
                            settings_row(
                                "workspace-settings-disconnect",
                                "Return to this device",
                                "Keep remote sessions running and switch to the local host.",
                                colors,
                            )
                            .child(
                                ui_button(
                                    "workspace-settings-disconnect-remote",
                                    ButtonKind::Secondary,
                                    colors,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.minimal_theme_open = false;
                                    this.disconnect_remote_runtime(cx);
                                }))
                                .child("Disconnect"),
                            ),
                        )
                    });
            }
            _ => {
                let copy = if cfg!(target_os = "macos") {
                    "Cmd C / Cmd V"
                } else {
                    "Ctrl C / Ctrl V"
                };
                content = content
                    .child(settings_heading("Conversation", colors))
                    .children(
                        [
                            ("Send message or queue while working", "Enter"),
                            ("New line", "Shift Enter"),
                            ("Copy / paste", copy),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, key))| {
                            div()
                                .id(("workspace-settings-chat-key", index))
                                .py_4()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .border_b_1()
                                .border_color(rgb(colors.border))
                                .child(div().text_size(px(13.0)).child(label))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(MONO)
                                        .text_size(px(11.5))
                                        .text_color(rgb(colors.muted))
                                        .child(key),
                                )
                        }),
                    )
                    .child(settings_heading("Terminal", colors))
                    .children(
                        [
                            ("Interrupt command", "Ctrl C"),
                            ("Copy selection", "Ctrl Shift C"),
                            (
                                "Paste",
                                if cfg!(target_os = "macos") {
                                    "Cmd V"
                                } else {
                                    "Ctrl Shift V"
                                },
                            ),
                            ("Clear terminal", "Ctrl L"),
                            ("Search command history", "Ctrl R"),
                            ("Close dialog", "Escape"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, key))| {
                            div()
                                .id(("workspace-settings-terminal-key", index))
                                .py_4()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .border_b_1()
                                .border_color(rgb(colors.border))
                                .child(div().text_size(px(13.0)).child(label))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(MONO)
                                        .text_size(px(11.5))
                                        .text_color(rgb(colors.muted))
                                        .child(key),
                                )
                        }),
                    );
            }
        }

        div()
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(colors.backdrop))
            .track_focus(&minimal_popup_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_minimal_theme_popup(window, cx)),
            )
            .child(
                div()
                    .id("workbench-settings")
                    .occlude()
                    .w(px(width))
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .rounded(px(16.0))
                    .overflow_hidden()
                    .bg(rgba(colors.chrome_glass))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(54.0))
                            .flex_none()
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(colors.text))
                                    .child("Settings"),
                            )
                            .child(
                                ui_button("workspace-settings-close", ButtonKind::Ghost, colors)
                                    .w(px(32.0))
                                    .px_0()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_minimal_theme_popup(window, cx)
                                    }))
                                    .child(icon(CLOSE_ICON, 16.0, colors.muted)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .flex()
                            .when(compact, |body| body.flex_col())
                            .child(navigation)
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}
