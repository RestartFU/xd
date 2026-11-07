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
    private val _resumeChats = MutableStateFlow(
        preferences.getBoolean(KEY_RESUME_CHATS, false),
    )

    /**
     * Continue the turns a host restart cut off, the next time this phone
     * connects. Off by default, as on the desktop: the agent starts working
     * without being asked.
     */
    public val resumeChats: StateFlow<Boolean> = _resumeChats.asStateFlow()

    public fun setResumeChats(enabled: Boolean) {
        preferences.edit().putBoolean(KEY_RESUME_CHATS, enabled).apply()
        _resumeChats.value = enabled
    }

    /**
     * The key of [accountKey]'s unsettled Codex reset attempt. Kept on disk
     * until the host settles it, so a retry after the app was killed settles
     * the same attempt instead of spending another credit.
     */
    public fun resetAttempt(accountKey: String): String? =
        preferences.getString(KEY_RESET_ATTEMPT + accountKey, null)

    /** Written before the request is sent, synchronously, so it cannot be lost. */
    public fun saveResetAttempt(accountKey: String, attempt: String) {
        preferences.edit().putString(KEY_RESET_ATTEMPT + accountKey, attempt).commit()
    }

    public fun clearResetAttempt(accountKey: String, attempt: String) {
        if (resetAttempt(accountKey) == attempt) {
            preferences.edit().remove(KEY_RESET_ATTEMPT + accountKey).apply()
        }
    }

    private val _theme = MutableStateFlow(
        MinimalThemePreset.fromKey(preferences.getString(KEY_THEME, null)),
    )

    public val accent: StateFlow<AccentPreset> = _accent.asStateFlow()
    public val speechEnabled: StateFlow<Boolean> = _speechEnabled.asStateFlow()
    public val allowAllPermissions: StateFlow<Boolean> = _allowAllPermissions.asStateFlow()
    public val theme: StateFlow<MinimalThemePreset> = _theme.asStateFlow()

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

    private companion object {
        const val PREFERENCES = "xd_mobile_settings"
        const val KEY_ACCENT = "accent"
        const val KEY_SPEECH_ENABLED = "speech_enabled"
        const val KEY_ALLOW_ALL_PERMISSIONS = "allow_all_permissions"
        const val KEY_THEME = "minimal_theme"
        const val KEY_RESUME_CHATS = "resume_chats"
        const val KEY_RESET_ATTEMPT = "usage_reset_attempt:"
    }
}
