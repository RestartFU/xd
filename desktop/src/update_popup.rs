use super::*;

/// One thing the popup says will change, or has just changed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UpdateRow {
    name: String,
    change: Option<String>,
    status: Option<String>,
    failed: bool,
}

/// Rolling channels identify a build by its full commit id.
fn short_version(version: &str) -> &str {
    if version.len() > 12 && version.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        &version[..7]
    } else {
        version
    }
}

fn version_change(current: Option<&str>, latest: Option<&str>) -> Option<String> {
    match (current, latest) {
        (Some(current), Some(latest)) if current != latest => Some(format!(
            "{} \u{2192} {}",
            short_version(current),
            short_version(latest)
        )),
        (Some(current), _) => Some(short_version(current).to_owned()),
        (None, latest) => latest.map(|latest| short_version(latest).to_owned()),
    }
}

impl SelfUpdatePanel {
    fn builds_from_source(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.channel == "dev")
    }

    /// Records the end of a source build, if it was the update's own.
    pub(super) fn finish_source_build(&mut self, result: &Result<(), String>) {
        if !std::mem::take(&mut self.source_building) {
            return;
        }
        match result {
            Ok(()) => self.source_installed = true,
            Err(error) => self.source_error = Some(error.clone()),
        }
    }

    fn app_state(&self) -> &str {
        if self.builds_from_source() {
            if self.source_building {
                return "installing";
            }
            if self.source_installed {
                return "installed";
            }
        }
        self.status
            .as_ref()
            .filter(|status| status.supported)
            .map_or("", |status| status.state.as_str())
    }

    fn app_pending(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.supported && status.available)
            && !matches!(self.app_state(), "installing" | "installed")
    }

    fn app_installed(&self) -> bool {
        self.app_state() == "installed"
    }

    fn pending_clis(&self) -> impl Iterator<Item = &CliUpdateStatus> {
        self.clis
            .iter()
            .filter(|cli| cli.available && cli.updatable && cli.state != "updating")
    }

    fn clis_updating(&self) -> bool {
        self.clis.iter().any(|cli| cli.state == "updating")
    }

    fn has_manual_update(&self) -> bool {
        self.clis.iter().any(|cli| cli.available && !cli.updatable)
    }

    fn has_pending_update(&self) -> bool {
        self.app_pending() || self.pending_clis().next().is_some()
    }

    fn update_in_progress(&self) -> bool {
        self.app_state() == "installing" || self.clis_updating()
    }

    /// Whether the catalog footer should offer the popup: there is something to
    /// confirm, something running, or an installed build waiting for a restart.
    pub(super) fn offers_update(&self) -> bool {
        self.has_pending_update()
            || self.has_manual_update()
            || self.update_in_progress()
            || self.app_installed()
    }

    fn update_rows(&self) -> Vec<UpdateRow> {
        let mut rows = Vec::new();
        if let Some(status) = self
            .status
            .as_ref()
            .filter(|status| status.supported)
            .filter(|_| {
                self.app_pending() || matches!(self.app_state(), "installing" | "installed")
            })
        {
            let installing = if self.builds_from_source() {
                "Building from source…"
            } else {
                "Installing…"
            };
            let (app, daemon) = match self.app_state() {
                "installing" => (Some(installing), Some(installing)),
                "installed" => (
                    Some("Installed. Restart to finish."),
                    Some("Installed. Restart to finish."),
                ),
                _ => (None, None),
            };
            let change = version_change(Some(&status.version), status.latest.as_deref());
            for (name, row_status) in [("xd app", app), ("xd daemon", daemon)] {
                rows.push(UpdateRow {
                    name: name.into(),
                    change: change.clone(),
                    status: row_status.map(str::to_owned),
                    failed: false,
                });
            }
        }
        rows.extend(
            self.clis
                .iter()
                .filter(|cli| {
                    cli.available || matches!(cli.state.as_str(), "updating" | "updated" | "failed")
                })
                .map(|cli| {
                    let updated = cli.state == "updated";
                    UpdateRow {
                        name: cli.display_name.clone(),
                        change: version_change(
                            cli.version.as_deref(),
                            if updated { None } else { cli.latest.as_deref() },
                        ),
                        status: match cli.state.as_str() {
                            "updating" => Some("Updating…".to_owned()),
                            "updated" => Some("Updated.".to_owned()),
                            "failed" => Some(
                                cli.detail
                                    .clone()
                                    .unwrap_or_else(|| "The update failed.".to_owned()),
                            ),
                            _ if cli.available && !cli.updatable => Some(
                                "This version cannot update itself. Update it the way you installed it."
                                    .to_owned(),
                            ),
                            _ => None,
                        },
                        failed: cli.state == "failed",
                    }
                }),
        );
        rows
    }

    fn update_summary(&self) -> &'static str {
        if self.app_installed() {
            "Restart xd to run the new version. Close terminal tabs and wait for chats to finish first."
        } else if self.builds_from_source() && self.app_pending() {
            "Confirm to build the latest master commit on this machine. It can take a while, and nothing restarts until you choose to."
        } else if self.has_pending_update() || self.update_in_progress() {
            "Confirm to install these. Nothing restarts until you choose to."
        } else if self.has_manual_update() {
            "xd cannot install this one for you."
        } else {
            "Everything is up to date."
        }
    }
}

impl XdDesktop {
    pub(super) fn apply_cli_updates(&mut self, value: &Value) {
        let Some(panel) = &mut self.self_update_panel else {
            return;
        };
        match serde_json::from_value::<Vec<CliUpdateStatus>>(
            value.get("providers").cloned().unwrap_or_default(),
        ) {
            Ok(statuses) => panel.clis = statuses,
            Err(error) => panel.error = Some(format!("Invalid assistant update reply: {error}")),
        }
    }

    pub(super) fn apply_cli_update(&mut self, value: &Value) {
        let Some(panel) = &mut self.self_update_panel else {
            return;
        };
        let Ok(status) = serde_json::from_value::<CliUpdateStatus>(value.clone()) else {
            return;
        };
        match panel
            .clis
            .iter_mut()
            .find(|existing| existing.provider == status.provider)
        {
            Some(existing) => *existing = status,
            None => panel.clis.push(status),
        }
    }

    pub(super) fn open_update_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_popup_open = true;
        self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        cx.notify();
    }

    pub(super) fn close_update_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_popup_open = false;
        self.restore_minimal_popup_focus(window);
        cx.notify();
    }

    /// Published channels install through the host. Nothing is published for
    /// master, so a dev build compiles the exact commit the user was shown here,
    /// even if master has moved on since.
    pub(super) fn install_app_update(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.self_update_panel.as_ref() else {
            return;
        };
        if !panel.builds_from_source() {
            self.request_app_update("install", cx);
            return;
        }
        let target = panel
            .status
            .as_ref()
            .and_then(|status| status.latest.as_deref())
            .and_then(source_build::parse_target);
        let refused = if self.source_build_panel.running {
            "A source build is already running. Wait for it or stop it in Settings."
        } else if let Some(target) = target {
            self.run_source_build(target, true, cx);
            return;
        } else {
            "Cannot tell which commit to build."
        };
        if let Some(panel) = self.self_update_panel.as_mut() {
            panel.source_error = Some(refused.into());
        }
        cx.notify();
    }

    /// Starts everything the popup listed. Each action is refused by the host
    /// if it is already running, so a repeated confirm cannot run one twice.
    fn confirm_updates(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.self_update_panel.as_mut() else {
            return;
        };
        if panel.busy {
            return;
        }
        panel.error = None;
        let install_app = panel.app_pending();
        let providers = panel
            .pending_clis()
            .map(|cli| cli.provider.clone())
            .collect::<Vec<_>>();
        if install_app {
            self.install_app_update(cx);
        }
        for provider in providers {
            let result = self
                .host
                .as_ref()
                .ok_or_else(|| {
                    "The local update service is reconnecting. Try again shortly.".to_owned()
                })
                .and_then(|host| host.agent_cli_updates("install", Some(&provider)));
            let Some(panel) = self.self_update_panel.as_mut() else {
                continue;
            };
            match result {
                Ok(()) => {
                    if let Some(cli) = panel.clis.iter_mut().find(|cli| cli.provider == provider) {
                        cli.state = "updating".into();
                        cli.detail = None;
                    }
                }
                Err(error) => panel.error = Some(error),
            }
        }
        cx.notify();
    }

    pub(super) fn render_update_popup(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let panel = self.self_update_panel.clone().unwrap_or_default();
        let pending = panel.has_pending_update();
        let in_progress = panel.update_in_progress();
        let installed = panel.app_installed();
        let can_update = pending && !panel.busy;
        let can_restart = installed && !panel.busy && !panel.clis_updating();
        let rows = panel.update_rows().into_iter().map(|row| {
            div()
                .py(px(10.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .border_b_1()
                .border_color(rgb(colors.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(13.0))
                                .text_color(rgb(colors.text))
                                .child(row.name),
                        )
                        .when_some(row.change, |line, change| {
                            line.child(
                                div()
                                    .flex_none()
                                    .font_family(MONO)
                                    .text_size(px(12.0))
                                    .text_color(rgb(colors.muted))
                                    .child(change),
                            )
                        }),
                )
                .when_some(row.status, |body, status| {
                    body.child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(rgb(if row.failed {
                                colors.danger
                            } else {
                                colors.muted
                            }))
                            .child(status),
                    )
                })
        });
        let keys = self.key_targets.clone();
        dialog_layer(colors)
            .track_focus(&self.minimal_popup_focus)
            .child(
                dialog_shell(DIALOG_WIDTH, colors)
                    .id("update-popup")
                    .child(dialog_title("Updates", colors))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.5))
                            .line_height(px(19.0))
                            .text_color(rgb(colors.muted))
                            .child(panel.update_summary()),
                    )
                    .child(
                        div()
                            .id("update-popup-items")
                            .min_h_0()
                            .max_h(px(300.0))
                            .overflow_y_scroll()
                            .children(rows)
                            .when_some(
                                panel.error.clone().or_else(|| panel.source_error.clone()),
                                |body, error| {
                                    body.child(
                                        div()
                                            .mt_3()
                                            .text_size(px(12.0))
                                            .line_height(px(18.0))
                                            .text_color(rgb(colors.danger))
                                            .child(error),
                                    )
                                },
                            ),
                    )
                    .child(
                        dialog_actions()
                            .child(dialog_button(
                                &keys,
                                "close-update-popup",
                                "Close",
                                ButtonKind::Ghost,
                                true,
                                colors,
                                cx,
                                |this, window, cx| this.close_update_popup(window, cx),
                            ))
                            .when(installed, |actions| {
                                actions.child(dialog_button(
                                    &keys,
                                    "restart-update-popup",
                                    "Restart to update",
                                    if pending {
                                        ButtonKind::Secondary
                                    } else {
                                        ButtonKind::Primary
                                    },
                                    can_restart,
                                    colors,
                                    cx,
                                    |this, _, cx| {
                                        if this.flush_draft_before_navigation() {
                                            this.request_app_update("restart", cx);
                                        }
                                    },
                                ))
                            })
                            .when(pending || (in_progress && !installed), |actions| {
                                actions.child(dialog_button(
                                    &keys,
                                    "confirm-update-popup",
                                    if pending { "Update" } else { "Updating…" },
                                    ButtonKind::Primary,
                                    can_update,
                                    colors,
                                    cx,
                                    |this, _, cx| this.confirm_updates(cx),
                                ))
                            }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const NEW_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn app(state: &str, available: bool) -> SelfUpdateStatus {
        SelfUpdateStatus {
            channel: "nightly".into(),
            state: state.into(),
            version: OLD_COMMIT.into(),
            latest: Some(NEW_COMMIT.into()),
            available,
            supported: true,
            error: None,
        }
    }

    fn cli(state: &str, available: bool) -> CliUpdateStatus {
        CliUpdateStatus {
            provider: "claude".into(),
            display_name: "Claude Code".into(),
            state: state.into(),
            version: Some("2.1.319".into()),
            latest: Some("2.1.330".into()),
            available,
            updatable: true,
            detail: None,
        }
    }

    fn panel(status: Option<SelfUpdateStatus>, clis: Vec<CliUpdateStatus>) -> SelfUpdatePanel {
        SelfUpdatePanel {
            status,
            clis,
            ..SelfUpdatePanel::default()
        }
    }

    #[gpui::test]
    fn the_footer_button_appears_with_an_update_and_opens_the_popup(cx: &mut gpui::TestAppContext) {
        install_embedded_fonts(cx.text_system()).expect("register bundled UI font");
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.settings.catalog_width = 256;
            cx.notify();
        });
        cx.run_until_parked();
        // The right end of the footer, where the Update button sits.
        let button = cx.update(|window, cx| {
            point(
                px(desktop.read(cx).catalog_width(window) - 40.0),
                window.viewport_size().height - px(26.0),
            )
        });

        cx.simulate_click(button, gpui::Modifiers::default());
        cx.update(|_, cx| assert!(!desktop.read(cx).update_popup_open));

        desktop.update(cx, |desktop, cx| {
            desktop.handle_event(
                "agent-cli-update",
                serde_json::json!({"provider":"claude", "display_name":"Claude Code",
                    "state":"idle", "version":"2.1.319", "latest":"2.1.330", "available":true}),
                None,
                cx,
            );
        });
        cx.run_until_parked();
        cx.simulate_click(button, gpui::Modifiers::default());
        cx.update(|_, cx| {
            let desktop = desktop.read(cx);
            assert!(desktop.update_popup_open);
            assert!(desktop.minimal_popup_is_open());
        });

        // The popup covers the window, so the footer no longer takes clicks.
        cx.simulate_click(button, gpui::Modifiers::default());
        cx.update(|_, cx| assert!(desktop.read(cx).update_popup_open));

        cx.dispatch_action(CloseSearch);
        cx.update(|_, cx| assert!(!desktop.read(cx).update_popup_open));
    }

    #[gpui::test]
    fn connecting_asks_for_local_state_from_either_endpoint(cx: &mut gpui::TestAppContext) {
        use std::{
            io::{BufRead, BufReader},
            net::{TcpListener, TcpStream},
        };

        // Loopback TCP stands in for the host, as Windows has no Unix sockets.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            for line in BufReader::new(stream).lines() {
                let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
                match request["op"].as_str().unwrap() {
                    "resume-interrupted" => seen.push(request["resume"].clone()),
                    "agent-cli-updates" => seen.push(request["action"].clone()),
                    _ => {}
                }
                if seen.len() == 4 {
                    break;
                }
            }
            seen
        });
        let stream = TcpStream::connect(address).unwrap();
        let (host, updates) =
            HostHandle::connect_io(stream.try_clone().unwrap(), stream, "test-host".into());
        let connected = updates.recv_blocking().unwrap();
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.host = Some(host);
            let generation = desktop.connection_generation;
            desktop.settings.auto_resume_chats = true;
            desktop.handle_host_update(connected.clone(), generation, cx);
            desktop.settings.auto_resume_chats = false;
            desktop.active_endpoint = ChatEndpoint::Remote;
            desktop.handle_host_update(connected, generation, cx);
        });
        assert_eq!(
            server.join().unwrap(),
            [
                serde_json::json!("status"),
                serde_json::json!(true),
                serde_json::json!("status"),
                serde_json::json!(false),
            ]
        );
    }

    #[gpui::test]
    fn updates_received_later_replace_the_listed_status(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, _| {
            desktop.apply_cli_updates(&serde_json::json!({"providers":[
                {"provider":"codex", "display_name":"Codex", "state":"idle", "available":false},
                {"provider":"claude", "display_name":"Claude Code", "state":"idle", "available":true},
            ]}));
            desktop.apply_cli_update(&serde_json::json!({"provider":"claude",
                "display_name":"Claude Code", "state":"updating", "available":true}));
            let panel = desktop.self_update_panel.as_ref().unwrap();
            assert_eq!(panel.clis.len(), 2);
            assert!(panel.clis_updating());
            assert!(!panel.has_pending_update());
        });
    }

    #[test]
    fn stays_hidden_with_nothing_to_update() {
        for quiet in [
            panel(None, Vec::new()),
            panel(Some(app("idle", false)), vec![cli("idle", false)]),
            panel(Some(app("checking", false)), Vec::new()),
            panel(Some(app("failed", false)), Vec::new()),
            panel(None, vec![cli("updated", false)]),
            panel(None, vec![cli("failed", false)]),
        ] {
            assert!(!quiet.offers_update());
        }
    }

    #[test]
    fn a_source_build_only_offers_its_clis() {
        let mut unsupported = app("idle", true);
        unsupported.supported = false;
        let source_build = panel(Some(unsupported.clone()), Vec::new());
        assert!(!source_build.offers_update());
        assert!(source_build.update_rows().is_empty());

        let with_cli = panel(Some(unsupported), vec![cli("idle", true)]);
        assert!(with_cli.offers_update());
        let rows = with_cli.update_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Claude Code");
        assert_eq!(rows[0].change.as_deref(), Some("2.1.319 → 2.1.330"));
    }

    #[test]
    fn lists_the_app_daemon_and_clis_with_their_versions() {
        let everything = panel(Some(app("idle", true)), vec![cli("idle", true)]);
        assert!(everything.offers_update());
        let rows = everything.update_rows();
        let names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["xd app", "xd daemon", "Claude Code"]);
        assert_eq!(rows[0].change.as_deref(), Some("0123456 → fedcba9"));
        assert_eq!(rows[1].change, rows[0].change);
        assert!(rows.iter().all(|row| row.status.is_none() && !row.failed));
    }

    #[test]
    fn an_up_to_date_cli_is_not_listed_beside_an_app_update() {
        let rows = panel(Some(app("idle", true)), vec![cli("idle", false)]).update_rows();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn follows_progress_through_install_and_restart() {
        let installing = panel(Some(app("installing", true)), vec![cli("updating", true)]);
        assert!(installing.offers_update());
        assert!(!installing.has_pending_update());
        assert!(installing.update_in_progress());
        let rows = installing.update_rows();
        assert_eq!(rows[0].status.as_deref(), Some("Installing…"));
        assert_eq!(rows[2].status.as_deref(), Some("Updating…"));

        let installed = panel(Some(app("installed", true)), vec![cli("updated", false)]);
        assert!(installed.offers_update());
        assert!(installed.app_installed());
        assert!(!installed.has_pending_update());
        let rows = installed.update_rows();
        assert_eq!(
            rows[0].status.as_deref(),
            Some("Installed. Restart to finish.")
        );
        assert_eq!(rows[2].status.as_deref(), Some("Updated."));
        assert_eq!(rows[2].change.as_deref(), Some("2.1.319"));
    }

    #[test]
    fn a_failed_cli_shows_why_and_can_be_retried_while_still_behind() {
        let mut failed = cli("failed", true);
        failed.detail = Some("no permission".into());
        let retry = panel(None, vec![failed]);
        assert!(retry.has_pending_update());
        let rows = retry.update_rows();
        assert!(rows[0].failed);
        assert_eq!(rows[0].status.as_deref(), Some("no permission"));
    }

    #[test]
    fn a_cli_without_an_updater_is_shown_but_not_offered_for_install() {
        let mut older = cli("idle", true);
        older.updatable = false;
        let manual = panel(None, vec![older]);
        assert!(manual.offers_update());
        assert!(!manual.has_pending_update());
        assert_eq!(
            manual.update_summary(),
            "xd cannot install this one for you."
        );
        let rows = manual.update_rows();
        assert!(
            rows[0]
                .status
                .as_deref()
                .unwrap()
                .contains("cannot update itself")
        );
    }

    #[test]
    fn a_dev_build_follows_its_source_build_instead_of_the_host() {
        let mut status = app("idle", true);
        status.channel = "dev".into();
        let mut dev = panel(Some(status), Vec::new());
        assert!(dev.app_pending());
        assert!(
            dev.update_summary()
                .contains("build the latest master commit")
        );

        dev.source_building = true;
        assert!(!dev.app_pending());
        assert!(dev.update_in_progress());
        assert_eq!(
            dev.update_rows()[0].status.as_deref(),
            Some("Building from source…")
        );

        dev.source_building = false;
        dev.source_installed = true;
        assert!(dev.app_installed());
        assert!(dev.offers_update());
        assert!(!dev.has_pending_update());
    }

    #[test]
    fn a_manual_source_build_is_not_taken_for_the_update() {
        let mut status = app("idle", true);
        status.channel = "dev".into();
        let mut dev = panel(Some(status), Vec::new());
        dev.finish_source_build(&Ok(()));
        assert!(!dev.source_installed);
        assert!(dev.app_pending());
        dev.finish_source_build(&Err("no docker".into()));
        assert_eq!(dev.source_error, None);

        dev.source_building = true;
        dev.finish_source_build(&Err("no docker".into()));
        assert_eq!(dev.source_error.as_deref(), Some("no docker"));
        assert!(!dev.source_installed);
        assert!(dev.app_pending());

        dev.source_building = true;
        dev.finish_source_build(&Ok(()));
        assert!(dev.app_installed());
    }

    #[test]
    fn other_channels_ignore_a_manual_source_build() {
        let mut nightly = panel(Some(app("idle", true)), Vec::new());
        nightly.source_building = true;
        assert!(nightly.app_pending());
        assert!(!nightly.update_in_progress());
    }

    #[test]
    fn keeps_semantic_versions_whole_and_shortens_commits() {
        assert_eq!(short_version(OLD_COMMIT), "0123456");
        assert_eq!(short_version("2.1.319"), "2.1.319");
        assert_eq!(short_version("development"), "development");
        assert_eq!(
            version_change(Some("1.0.0"), Some("1.0.0")).as_deref(),
            Some("1.0.0")
        );
        assert_eq!(version_change(None, None), None);
    }
}
