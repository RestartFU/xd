package com.restartfu.xd.mobile

import android.app.Activity
import android.app.UiModeManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import com.restartfu.xd.mobile.ui.FatalScreen
import com.restartfu.xd.mobile.ui.MinimalThemePreset
import com.restartfu.xd.mobile.ui.navigationBarStyle
import com.restartfu.xd.mobile.ui.systemBarStyle
import com.restartfu.xd.mobile.ui.MinimalMobileApp
import com.restartfu.xd.mobile.ui.ConnectScreen
import com.restartfu.xd.mobile.ui.XdRoot
import com.restartfu.xd.net.Link

class MainActivity : ComponentActivity() {
    private val model: MainViewModel by viewModels()
    private val settings: MobileSettings by lazy {
        (application as XdApplication).settings
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before the first frame: the app draws behind the system bars on every
        // Android version, as Android 15 already makes it, in the saved theme.
        enableEdgeToEdge(settings.theme.value)
        super.onCreate(savedInstanceState)
        // The window the system shows while the app starts wears the saved
        // theme's ground from the next launch on, not the system's night mode.
        lifecycleScope.launch { settings.theme.collect(::launchInTheme) }
        setContent {
            val theme by settings.theme.collectAsStateWithLifecycle()
            // The bars' icons follow the app's theme, not the system setting.
            DisposableEffect(theme) {
                enableEdgeToEdge(theme)
                onDispose {}
            }
            XdRoot(theme) {
                XdMobileApp(model, settings)
            }
        }
    }

    /**
     * Tells Android which night mode the app is in, so the launch window
     * (`res/values-night`) matches [theme] next time. Android 12 and later
     * keep this per app; before it, the system's own night mode still picks.
     * The activity takes the change itself (`configChanges="uiMode"`), so
     * changing the theme does not restart it.
     */
    private fun launchInTheme(theme: MinimalThemePreset) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
        val modes = getSystemService(UiModeManager::class.java) ?: return
        modes.setApplicationNightMode(launchNightMode(theme))
    }

    private fun enableEdgeToEdge(theme: MinimalThemePreset) {
        enableEdgeToEdge(statusBarStyle = theme.systemBarStyle(), navigationBarStyle = theme.navigationBarStyle())
    }
}

/** The night mode whose launch window wears [theme]'s ground. */
internal fun launchNightMode(theme: MinimalThemePreset): Int =
    if (theme.isLight) UiModeManager.MODE_NIGHT_NO else UiModeManager.MODE_NIGHT_YES

@Composable
private fun XdMobileApp(
    model: MainViewModel,
    settings: MobileSettings,
) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val activity = LocalContext.current as? Activity
    val hasCredentials by model.client.hasCredentials.collectAsStateWithLifecycle()
    val credentialsReady by model.client.credentialsReady.collectAsStateWithLifecycle()
    val link by model.client.link.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()

    // No foreground service: leaving closes the socket, returning reconnects
    // and re-snapshots. The host keeps no resumable event log, so there is
    // nothing a backgrounded socket could usefully catch up on.
    // While the app is in front, a network coming back is a reason to try
    // the host at once instead of waiting out the backoff.
    val context = LocalContext.current
    val networkWatch = remember(context, model.client) { NetworkReturnWatch(context, model.client::retryConnection) }
    DisposableEffect(lifecycleOwner, model.client) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> {
                    model.client.poke()
                    networkWatch.start()
                }
                Lifecycle.Event.ON_STOP -> {
                    networkWatch.stop()
                    if (activity?.isChangingConfigurations != true) {
                        model.client.goBackground()
                    }
                }
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            networkWatch.stop()
        }
    }

    if (!credentialsReady) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator()
        }
    } else if (!hasCredentials) {
        ConnectScreen(model)
    } else if (link is Link.Fatal) {
        FatalScreen(link as Link.Fatal, operationError, retry = model.client::retryConnection, forget = model::forget)
    } else {
        MinimalMobileApp(model, link, settings)
    }
}
