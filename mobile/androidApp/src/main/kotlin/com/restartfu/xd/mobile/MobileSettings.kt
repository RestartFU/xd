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
    var usageMetric: String
        get() = preferences.getString("usage_metric", "Limits")?.takeIf { it in listOf("Cost", "Tokens", "Limits") } ?: "Limits"
        set(value) { preferences.edit().putString("usage_metric", value).apply() }
    var usageProvider: String
        get() = preferences.getString("usage_provider", "All providers")?.takeIf { it in listOf("All providers", "Codex", "Claude") } ?: "All providers"
        set(value) { preferences.edit().putString("usage_provider", value).apply() }
    var usageDays: Int
        get() = preferences.getInt("usage_days", 30).takeIf { it in listOf(1, 7, 30, 90) } ?: 30
        set(value) { preferences.edit().putInt("usage_days", value).apply() }

    private val _accent = MutableStateFlow(
        AccentPreset.fromKey(preferences.getString(KEY_ACCENT, null)),
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

    /**
     * Written synchronously before the request is sent; false if it could
     * not be stored, in which case nothing may be sent.
     */
    public fun saveResetAttempt(accountKey: String, attempt: String): Boolean =
        preferences.edit().putString(KEY_RESET_ATTEMPT + accountKey, attempt).commit()

    public fun clearResetAttempt(accountKey: String, attempt: String) {
        if (resetAttempt(accountKey) == attempt) {
            preferences.edit().remove(KEY_RESET_ATTEMPT + accountKey).apply()
        }
    }

    private val _collapsedFolders = MutableStateFlow(
        preferences.getStringSet(KEY_COLLAPSED_FOLDERS, null).orEmpty().toSet(),
    )

    /**
     * Folders and projects closed on the Projects page; the rest are open, as
     * on the desktop, so any number can be open at once.
     */
    public val collapsedFolders: StateFlow<Set<String>> = _collapsedFolders.asStateFlow()

    private val _lastSessionProject = MutableStateFlow(preferences.getString(KEY_LAST_SESSION_PROJECT, null))

    /** The project the last session was started in, offered first next time. */
    public val lastSessionProject: StateFlow<String?> = _lastSessionProject.asStateFlow()

    public fun setLastSessionProject(projectId: String) {
        if (_lastSessionProject.value == projectId) return
        preferences.edit().putString(KEY_LAST_SESSION_PROJECT, projectId).apply()
        _lastSessionProject.value = projectId
    }

    public fun setFolderOpen(folderId: String, open: Boolean) {
        val next = if (open) _collapsedFolders.value - folderId else _collapsedFolders.value + folderId
        if (next == _collapsedFolders.value) return
        preferences.edit().putStringSet(KEY_COLLAPSED_FOLDERS, next).apply()
        _collapsedFolders.value = next
    }

    private val _theme = MutableStateFlow(
        MinimalThemePreset.fromKey(preferences.getString(KEY_THEME, null)),
    )

    public val accent: StateFlow<AccentPreset> = _accent.asStateFlow()
    public val allowAllPermissions: StateFlow<Boolean> = _allowAllPermissions.asStateFlow()
    public val theme: StateFlow<MinimalThemePreset> = _theme.asStateFlow()

    public fun setAccent(preset: AccentPreset) {
        preferences.edit().putString(KEY_ACCENT, preset.key).apply()
        _accent.value = preset
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
        const val KEY_ALLOW_ALL_PERMISSIONS = "allow_all_permissions"
        const val KEY_THEME = "minimal_theme"
        const val KEY_COLLAPSED_FOLDERS = "collapsed_folders"
        const val KEY_LAST_SESSION_PROJECT = "last_session_project"
        const val KEY_RESUME_CHATS = "resume_chats"
        const val KEY_RESET_ATTEMPT = "usage_reset_attempt:"
    }
}
