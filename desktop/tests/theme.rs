use xd_desktop::theme::ThemePreset;

#[test]
fn theme_presets_have_stable_names_and_default_to_dark() {
    assert_eq!(ThemePreset::default(), ThemePreset::Dark);
    assert_eq!(
        ThemePreset::ALL,
        [
            ThemePreset::Dark,
            ThemePreset::Light,
            ThemePreset::Warm,
            ThemePreset::Nord,
            ThemePreset::Dracula,
        ]
    );
    assert_eq!(ThemePreset::Dark.label(), "Dark");
    assert_eq!(ThemePreset::Light.label(), "Light");
    assert_eq!(ThemePreset::Warm.label(), "Warm");
    assert_eq!(ThemePreset::Nord.label(), "Nord");
    assert_eq!(ThemePreset::Dracula.label(), "Dracula");

    assert_eq!(
        serde_json::to_string(&ThemePreset::Dark).unwrap(),
        r#""dark""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Light).unwrap(),
        r#""light""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Warm).unwrap(),
        r#""warm""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Nord).unwrap(),
        r#""nord""#
    );
    assert_eq!(
        serde_json::to_string(&ThemePreset::Dracula).unwrap(),
        r#""dracula""#
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
