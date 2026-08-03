package com.restartfu.xd.mobile

import android.content.Context
import com.restartfu.xd.mobile.ui.AccentPreset
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

    public val accent: StateFlow<AccentPreset> = _accent.asStateFlow()
    public val speechEnabled: StateFlow<Boolean> = _speechEnabled.asStateFlow()

    public fun setAccent(preset: AccentPreset) {
        preferences.edit().putString(KEY_ACCENT, preset.key).apply()
        _accent.value = preset
    }

    public fun setSpeechEnabled(enabled: Boolean) {
        preferences.edit().putBoolean(KEY_SPEECH_ENABLED, enabled).apply()
        _speechEnabled.value = enabled
    }

    private companion object {
        const val PREFERENCES = "xd_mobile_settings"
        const val KEY_ACCENT = "accent"
        const val KEY_SPEECH_ENABLED = "speech_enabled"
    }
}
