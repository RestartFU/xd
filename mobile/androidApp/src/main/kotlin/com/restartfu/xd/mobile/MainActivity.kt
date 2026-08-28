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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ui.FatalScreen
import com.restartfu.xd.mobile.ui.MinimalMobileApp
import com.restartfu.xd.mobile.ui.ConnectScreen
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
                Surface(Modifier.fillMaxSize()) {
                    XdMobileApp(model, settings)
                }
            }
        }
    }
}

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
        ConnectScreen(model)
    } else if (link is Link.Fatal) {
        FatalScreen(link as Link.Fatal, operationError, model::forget)
    } else {
        MinimalMobileApp(model, link, settings)
    }
}
