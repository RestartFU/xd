package com.restartfu.xd.mobile

import android.content.Context
import com.restartfu.xd.mobile.ui.AccentPreset
import com.restartfu.xd.mobile.ui.MinimalThemePreset
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Global settings shared by every mobile screen and persisted across launches. */
public class MobileSettings(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES,
        Context.MODE_PRIVATE,
    )
    private val _accent = MutableStateFlow(
        AccentPreset.fromKey(preferences.getString(KEY_ACCENT, null)),
    )
    private val _speechEnabled = MutableStateFlow(
        preferences.getBoolean(KEY_SPEECH_ENABLED, false),
    )
    private val _allowAllPermissions = MutableStateFlow(
        preferences.getBoolean(KEY_ALLOW_ALL_PERMISSIONS, false),
    )
    private val _theme = MutableStateFlow(
        MinimalThemePreset.fromKey(preferences.getString(KEY_THEME, null)),
    )
    private val _experimentMode = MutableStateFlow(
        preferences.getBoolean(KEY_EXPERIMENT_MODE, false),
    )

    public val accent: StateFlow<AccentPreset> = _accent.asStateFlow()
    public val speechEnabled: StateFlow<Boolean> = _speechEnabled.asStateFlow()
    public val allowAllPermissions: StateFlow<Boolean> = _allowAllPermissions.asStateFlow()
    public val theme: StateFlow<MinimalThemePreset> = _theme.asStateFlow()
    public val experimentMode: StateFlow<Boolean> = _experimentMode.asStateFlow()

    public fun setAccent(preset: AccentPreset) {
        preferences.edit().putString(KEY_ACCENT, preset.key).apply()
        _accent.value = preset
    }

    public fun setSpeechEnabled(enabled: Boolean) {
        preferences.edit().putBoolean(KEY_SPEECH_ENABLED, enabled).apply()
        _speechEnabled.value = enabled
    }

    public fun setAllowAllPermissions(enabled: Boolean) {
        preferences.edit().putBoolean(KEY_ALLOW_ALL_PERMISSIONS, enabled).apply()
        _allowAllPermissions.value = enabled
    }

    public fun setTheme(theme: MinimalThemePreset) {
        preferences.edit().putString(KEY_THEME, theme.key).apply()
        _theme.value = theme
    }

    public fun setExperimentMode(enabled: Boolean) {
        preferences.edit().putBoolean(KEY_EXPERIMENT_MODE, enabled).apply()
        _experimentMode.value = enabled
    }

    private companion object {
        const val PREFERENCES = "xd_mobile_settings"
        const val KEY_ACCENT = "accent"
        const val KEY_SPEECH_ENABLED = "speech_enabled"
        const val KEY_ALLOW_ALL_PERMISSIONS = "allow_all_permissions"
        const val KEY_THEME = "minimal_theme"
        const val KEY_EXPERIMENT_MODE = "experiment_mode"
    }
}
