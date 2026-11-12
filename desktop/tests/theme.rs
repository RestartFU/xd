use xd_desktop::theme::{Glass, ThemePreset};

#[test]
fn theme_presets_have_stable_names_and_default_to_dark() {
    assert_eq!(ThemePreset::default(), ThemePreset::Dark);
    assert_eq!(
        ThemePreset::ALL,
        [
            ThemePreset::Dark,
            ThemePreset::Light,
            ThemePreset::Modern,
            ThemePreset::ModernLight
        ]
    );
    assert_eq!(ThemePreset::Dark.label(), "Dark");
    assert_eq!(ThemePreset::Light.label(), "Light");
    assert_eq!(ThemePreset::Modern.label(), "Modern");
    assert_eq!(ThemePreset::ModernLight.label(), "Modern Light");

    assert_eq!(
        serde_json::to_string(&ThemePreset::Dark).unwrap(),
        r#""dark""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Light).unwrap(),
        r#""light""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Modern).unwrap(),
        r#""modern""#
    );
    assert_eq!(
        serde_json::from_str::<ThemePreset>(r#""modern""#).unwrap(),
        ThemePreset::Modern
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::ModernLight).unwrap(),
        r#""modern-light""#
    );
    assert_eq!(
        serde_json::from_str::<ThemePreset>(r#""modern-light""#).unwrap(),
        ThemePreset::ModernLight
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
fn only_the_modern_themes_are_glass() {
    for preset in ThemePreset::ALL {
        let glass = preset.colors().glass;
        assert_eq!(
            glass.is_some(),
            matches!(preset, ThemePreset::Modern | ThemePreset::ModernLight),
            "{preset:?}"
        );
        if let Some(glass) = glass {
            // The wash is the window's own paint; only the panes over it let it through.
            assert!(glass.wash_from <= 0xff_ffff && glass.wash_to <= 0xff_ffff);
            for (role, fill) in [
                ("pane", glass.pane),
                ("composer", glass.composer),
                ("glow", glass.glow),
            ] {
                let alpha = fill & 0xff;
                assert!(
                    alpha > 0 && alpha < 0xff,
                    "{preset:?} {role} is not translucent"
                );
            }
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

/// Text sits straight on the wash in a page canvas, and on the panes and the
/// composer laid over it, so it has to read at both ends of the wash.
#[test]
fn text_reads_on_every_glass_ground() {
    for preset in ThemePreset::ALL {
        let colors = preset.colors();
        let Some(glass) = colors.glass else { continue };
        // The glow can reach any part of the wash, so check with and without it.
        for (end, wash) in [
            ("top left", glass.wash_from),
            ("bottom right", glass.wash_to),
            (
                "top left under the glow",
                Glass::over(glass.glow, glass.wash_from),
            ),
            (
                "bottom right under the glow",
                Glass::over(glass.glow, glass.wash_to),
            ),
        ] {
            for (ground, background) in [
                ("wash", wash),
                ("pane", Glass::over(glass.pane, wash)),
                ("composer", Glass::over(glass.composer, wash)),
            ] {
                let place = format!("{preset:?} {ground} at the {end}");
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
