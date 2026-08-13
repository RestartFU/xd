package com.restartfu.xd.mobile

import android.app.Activity
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ui.FatalScreen
import com.restartfu.xd.mobile.ui.MinimalMobileApp
import com.restartfu.xd.mobile.ui.PairScreen
import com.restartfu.xd.mobile.ui.minimalColors
import com.restartfu.xd.net.Link

class MainActivity : ComponentActivity() {
    private val model: MainViewModel by viewModels()
    private val settings: MobileSettings by lazy {
        (application as XdApplication).settings
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val theme by settings.theme.collectAsStateWithLifecycle()
            MaterialTheme(colorScheme = minimalColors(theme)) {
                Compact {
                    Surface(Modifier.fillMaxSize()) {
                        XdMobileApp(model, settings)
                    }
                }
            }
        }
    }
}

/**
 * Draws the app slightly smaller so more projects, sessions, and terminal fit.
 *
 * Scaling density rather than the type ramp shrinks padding, icons and row
 * heights along with the text; making only the text smaller would leave the
 * whitespace around it and win far less room.
 *
 * fontScale is passed through untouched, so someone who has asked the system
 * for larger text still gets it, just within a tighter layout.
 */
@Composable
private fun Compact(content: @Composable () -> Unit) {
    val density = LocalDensity.current
    CompositionLocalProvider(
        LocalDensity provides Density(
            density = density.density * COMPACT_SCALE,
            fontScale = density.fontScale,
        ),
        content = content,
    )
}

// 7/8. Enough to gain about one row in eight, while a 48dp touch target still
// lands near 42dp, which stays comfortably tappable.
private const val COMPACT_SCALE = 0.875f

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
    DisposableEffect(lifecycleOwner, model.client) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> model.client.poke()
                Lifecycle.Event.ON_STOP -> {
                    if (activity?.isChangingConfigurations != true) {
                        model.client.goBackground()
                    }
                }
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    if (!credentialsReady) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator()
        }
    } else if (!hasCredentials) {
        PairScreen(model)
    } else if (link is Link.Fatal) {
        FatalScreen(link as Link.Fatal, operationError, model::forget)
    } else {
        MinimalMobileApp(model, link, settings)
    }
}
