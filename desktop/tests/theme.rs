use xd_desktop::theme::{Glass, ThemePreset};

#[test]
fn theme_presets_have_stable_names_and_default_to_dark() {
    assert_eq!(ThemePreset::default(), ThemePreset::Dark);
    assert_eq!(ThemePreset::ALL, [ThemePreset::Dark, ThemePreset::Light]);
    assert_eq!(ThemePreset::Dark.label(), "Dark");
    assert_eq!(ThemePreset::Light.label(), "Light");

    assert_eq!(
        serde_json::to_string(&ThemePreset::Dark).unwrap(),
        r#""dark""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Light).unwrap(),
        r#""light""#
    );
}

#[test]
fn every_preset_exposes_a_complete_readable_semantic_palette() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();

        assert_ne!(colors.background, colors.surface, "{preset:?}");
        assert_ne!(colors.surface, colors.surface_high, "{preset:?}");
        assert_ne!(colors.selected_surface, colors.surface, "{preset:?}");
        assert_ne!(colors.selected_border, colors.border, "{preset:?}");
        assert_ne!(colors.text, colors.muted, "{preset:?}");
        assert!(
            contrast(colors.text, colors.background) >= 7.0,
            "{preset:?}"
        );
        assert!(
            contrast(colors.muted, colors.background) >= 4.5,
            "{preset:?}"
        );
        assert!(
            contrast(colors.accent_text, colors.accent) >= 4.5,
            "{preset:?}"
        );
        assert!(
            contrast(colors.text, colors.selected_surface) >= 7.0,
            "{preset:?}"
        );
        assert!(
            contrast(colors.muted, colors.selected_surface) >= 4.5,
            "{preset:?}"
        );
        assert!(
            contrast(colors.accent_text, colors.accent_hover) >= 4.5,
            "{preset:?}"
        );
        assert!(
            contrast(colors.accent_ink, colors.background) >= 4.5,
            "{preset:?}"
        );
        assert!(
            contrast(colors.accent_ink, colors.selected_surface) >= 4.5,
            "{preset:?}"
        );
    }
}

#[test]
fn theme_controls_remain_readable_on_every_surface() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();

        for (role, background) in [
            ("sidebar", colors.sidebar),
            ("surface", colors.surface),
            ("surface_high", colors.surface_high),
            ("chrome", colors.chrome),
            ("composer", colors.composer),
        ] {
            assert!(
                contrast(colors.text, background) >= 7.0,
                "{preset:?} text on {role}"
            );
            assert!(
                contrast(colors.muted, background) >= 4.5,
                "{preset:?} muted text on {role}"
            );
            assert!(
                contrast(colors.accent_ink, background) >= 4.5,
                "{preset:?} accent ink on {role}"
            );
        }
    }
}

#[test]
fn window_surfaces_are_opaque_rgb_roles() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (role, color) in [
            ("background", colors.background),
            ("sidebar", colors.sidebar),
            ("chrome", colors.chrome),
            ("composer", colors.composer),
        ] {
            assert!(color <= 0xff_ffff, "{preset:?} {role} carries alpha");
        }
    }
}

#[test]
fn signal_colors_read_as_text_on_every_surface() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (role, background) in [
            ("background", colors.background),
            ("sidebar", colors.sidebar),
            ("surface", colors.surface),
            ("surface_high", colors.surface_high),
            ("selected_surface", colors.selected_surface),
        ] {
            for (signal, color) in [
                ("working", colors.working),
                ("waiting", colors.waiting),
                ("done", colors.done),
                ("danger", colors.danger),
            ] {
                assert!(
                    contrast(color, background) >= 4.5,
                    "{preset:?} {signal} on {role}"
                );
            }
            assert!(
                contrast(colors.faint, background) >= 3.0,
                "{preset:?} faint on {role}"
            );
        }
        assert!(contrast(colors.danger, colors.danger_surface) >= 4.5);
        assert_ne!(colors.waiting, colors.working, "{preset:?}");
    }
}

#[test]
fn every_theme_is_translucent_glass_over_an_opaque_backdrop() {
    for preset in ThemePreset::ALL {
        let glass = preset.colors().glass;
        // The backdrop is the window's own paint; only the panes over it let it through.
        assert!(
            glass.backdrop <= 0xff_ffff,
            "{preset:?} backdrop carries alpha"
        );
        for (role, fill) in [("pane", glass.pane), ("composer", glass.composer)] {
            let alpha = fill & 0xff;
            assert!(
                alpha > 0 && alpha < 0xff,
                "{preset:?} {role} is not translucent"
            );
        }
    }
}

#[test]
fn glass_blends_a_translucent_fill_over_what_is_beneath() {
    assert_eq!(Glass::over(0xffffffff, 0x000000), 0xffffff);
    assert_eq!(Glass::over(0xffffff00, 0x123456), 0x123456);
    assert_eq!(Glass::over(0xffffff80, 0x000000), 0x808080);
    assert_eq!(Glass::over(0x20406080, 0x604020), 0x404040);
}

/// Text sits straight on the backdrop in a page canvas, and on the panes and
/// the composer laid over it.
#[test]
fn text_reads_on_every_glass_ground() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        let glass = colors.glass;
        for (ground, background) in [
            ("backdrop", glass.backdrop),
            ("pane", Glass::over(glass.pane, glass.backdrop)),
            ("composer", Glass::over(glass.composer, glass.backdrop)),
        ] {
            let place = format!("{preset:?} {ground}");
            assert!(contrast(colors.text, background) >= 7.0, "text on {place}");
            for (role, color) in [
                ("muted", colors.muted),
                ("accent ink", colors.accent_ink),
                ("working", colors.working),
                ("waiting", colors.waiting),
                ("done", colors.done),
                ("danger", colors.danger),
            ] {
                assert!(contrast(color, background) >= 4.5, "{role} on {place}");
            }
            assert!(
                contrast(colors.faint, background) >= 3.0,
                "faint on {place}"
            );
        }
    }
}

#[test]
fn selectable_themes_produce_distinct_palettes() {
    let palettes = ThemePreset::ALL.map(ThemePreset::colors);

    for (index, palette) in palettes.iter().enumerate() {
        for other in &palettes[index + 1..] {
            assert_ne!(palette, other);
        }
    }
}

/// The grounds a control or a scroll area is drawn on in a theme: the
/// opaque roles, and the glass panes and composer over the window's backdrop.
fn grounds(colors: xd_desktop::theme::ThemeColors) -> Vec<(&'static str, u32)> {
    let glass = colors.glass;
    vec![
        ("background", colors.background),
        ("sidebar", colors.sidebar),
        ("surface", colors.surface),
        ("surface_high", colors.surface_high),
        ("selected_surface", colors.selected_surface),
        ("chrome", colors.chrome),
        ("composer", colors.composer),
        ("glass backdrop", glass.backdrop),
        ("glass pane", Glass::over(glass.pane, glass.backdrop)),
        (
            "glass composer",
            Glass::over(glass.composer, glass.backdrop),
        ),
    ]
}

#[test]
fn scroll_thumbs_and_control_edges_read_at_three_to_one_on_every_ground() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (ground, color) in grounds(colors) {
            // The thumb is translucent: what shows is it over the ground.
            let thumb = Glass::over(colors.scroll_thumb, color);
            let ratio = contrast(thumb, color);
            assert!(
                ratio >= 3.0,
                "{preset:?} scroll thumb on {ground} is {ratio:.2}:1"
            );
            let ratio = contrast(colors.control_border, color);
            assert!(
                ratio >= 3.0,
                "{preset:?} control edge on {ground} is {ratio:.2}:1"
            );
        }
        // The hairlines between panes stay quiet: under the control edge.
        assert!(
            contrast(colors.border, colors.background)
                < contrast(colors.control_border, colors.background),
            "{preset:?}"
        );
    }
}

/// The bar that marks the selected row, tab, segment or section is the one
/// sign of selection that reads at 3:1: the selected fill stays quiet.
#[test]
fn the_selection_marker_reads_at_three_to_one_on_every_ground() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (ground, color) in grounds(colors) {
            let ratio = contrast(colors.selected_border, color);
            assert!(
                ratio >= 3.0,
                "{preset:?} selection marker on {ground} is {ratio:.2}:1"
            );
        }
        // The fill alone does not carry it.
        assert!(
            contrast(colors.selected_surface, colors.sidebar) < 3.0,
            "{preset:?}"
        );
    }
}

/// The row the arrow keys are on in a menu, a picker or a list carries a
/// ring in the selection colour, which hover never draws: at 3:1 or more on
/// the menu and on the lit fill the row takes, so it is seen apart from a
/// row that is only lit.
#[test]
fn the_arrow_keys_ring_reads_at_three_to_one_on_a_menu_and_its_lit_row() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (ground, color) in [
            ("menu", colors.surface),
            ("lit row", colors.surface_high),
            ("lit answer", colors.selected_surface),
        ] {
            let ratio = contrast(colors.selected_border, color);
            assert!(ratio >= 3.0, "{preset:?} ring on {ground} is {ratio:.2}:1");
        }
        // The lit fill alone is the pointer's, and does not carry it.
        assert!(contrast(colors.surface_high, colors.surface) < 3.0);
    }
}

/// A control that cannot be used now is readable, and is told apart from
/// one that can. Its words stay at 4.5:1 or more on every ground. In Dark
/// they are also 1.5:1 or more from the muted ink of a quiet control that
/// can be used. Light has no such ink: anything that far lighter than its
/// muted falls under 4.5:1 on its paler grounds, as checked here, so there
/// the dashed edge a disabled control takes, and no usable control has,
/// tells them apart.
#[test]
fn disabled_words_read_and_stand_apart_from_a_usable_controls() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        for (ground, color) in grounds(colors) {
            let ratio = contrast(colors.disabled, color);
            assert!(
                ratio >= 4.5,
                "{preset:?} disabled words on {ground} are {ratio:.2}:1"
            );
        }
        // Its edge, dashed, shows where the control is.
        assert!(contrast(colors.control_border, colors.surface) >= 3.0);
    }
    let dark = ThemePreset::Dark.colors();
    let apart = contrast(dark.disabled, dark.muted);
    assert!(apart >= 1.5, "Dark disabled is {apart:.2}:1 from muted");
    let light = ThemePreset::Light.colors();
    let lighter_than_muted = (0..=0xff_u32)
        .map(|value| (value << 16) | (value << 8) | value)
        .filter(|&grey| luminance(grey) > luminance(light.muted))
        .filter(|&grey| contrast(grey, light.muted) >= 1.5);
    for grey in lighter_than_muted {
        assert!(
            grounds(light)
                .iter()
                .any(|&(_, ground)| contrast(grey, ground) < 4.5),
            "#{grey:06x} would do in Light"
        );
    }
}

/// A working dot pulses, but never so faintly that it falls under 3:1 on
/// the grounds it is drawn on.
#[test]
fn a_pulsing_dot_stays_at_three_to_one_at_its_faintest() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        let faintest = (colors.working << 8) | u32::from(colors.pulse_floor);
        for (ground, color) in grounds(colors) {
            let ratio = contrast(Glass::over(faintest, color), color);
            assert!(
                ratio >= 3.0,
                "{preset:?} working dot at its faintest on {ground} is {ratio:.2}:1"
            );
        }
    }
}

fn contrast(foreground: u32, background: u32) -> f64 {
    let foreground = luminance(foreground);
    let background = luminance(background);
    let lighter = foreground.max(background);
    let darker = foreground.min(background);
    (lighter + 0.05) / (darker + 0.05)
}

fn luminance(color: u32) -> f64 {
    let channel = |shift| {
        let value = f64::from((color >> shift) & 0xff_u32) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };

    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}
