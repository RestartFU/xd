package com.restartfu.xd.mobile.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.ui.Alignment
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.TextButton
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.material3.LocalContentColor
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link

@Composable
internal fun ConnectScreen(model: MainViewModel) {
    // Saved, so turning the phone does not empty a form half filled in.
    var address by rememberSaveable { mutableStateOf("") }
    var pairingCode by rememberSaveable { mutableStateOf("") }
    val connecting by model.connecting.collectAsStateWithLifecycle()
    val error by model.connectError.collectAsStateWithLifecycle()
    val ready = !connecting && address.isNotBlank() && pairingCode.isNotBlank()
    val pair = { if (ready) model.connectPairing(address, pairingCode) }
    StandalonePage {
        Text("Pair with xd", modifier = Modifier.semantics { heading() }, style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(12.dp))
        Text("On your host, open Settings → Pair a device. Copy its pairing code here, and enter a LAN or Tailscale address that reaches the host.")
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = address,
            onValueChange = { address = it },
            modifier = Modifier.fillMaxWidth().testTag(PAIRING_ADDRESS_TAG),
            label = { Text("Host address") },
            singleLine = true,
            enabled = !connecting,
            // An address, not prose: the keyboard has its ".", "/" and ":" to
            // hand, corrects nothing, and its action key moves on to the code.
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Next,
            ),
            colors = xdFieldColors(),
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = pairingCode,
            onValueChange = { pairingCode = it },
            modifier = Modifier.fillMaxWidth().testTag(PAIRING_CODE_TAG),
            label = { Text("Pairing code") },
            enabled = !connecting,
            // Said on the field it is about, and told as it appears: the form
            // gives no other sign that pairing failed.
            isError = error != null,
            supportingText = error?.let { message ->
                { Text(message, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
            },
            // A code is typed or pasted exactly; the action key pairs.
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Ascii,
                imeAction = ImeAction.Done,
            ),
            keyboardActions = KeyboardActions(onDone = { pair() }),
            colors = xdFieldColors(),
        )
        Spacer(Modifier.height(12.dp))
        Button(onClick = pair, enabled = ready, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
            if (connecting) {
                // Says what it is doing, at the size the button keeps throughout.
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = LocalContentColor.current)
                Spacer(Modifier.width(10.dp))
                Text(CONNECTING_LABEL)
            } else {
                Text("Pair and connect")
            }
        }
    }
}

/** What the pairing button says while it connects. */
internal const val CONNECTING_LABEL = "Connecting\u2026"

/** The pairing form's two fields, for tests. */
internal const val PAIRING_ADDRESS_TAG = "pairing-address"
internal const val PAIRING_CODE_TAG = "pairing-code"

/**
 * A page shown on its own, before there is a host to show anything else: clear
 * of the system bars, a display cutout and the keyboard on every side, and no
 * wider than a form reads well at, in the middle of a wide window.
 */
@Composable
private fun StandalonePage(content: @Composable ColumnScope.() -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .verticalScroll(rememberScrollState()),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier.widthIn(max = FormMeasure).fillMaxWidth().padding(24.dp),
            verticalArrangement = Arrangement.Center,
            content = content,
        )
    }
}

/** The widest a form of a few fields is drawn. */
private val FormMeasure = 480.dp

/**
 * The host cannot be reached with this pairing. A failure another attempt can
 * get past offers Try again first, and asks before forgetting a pairing that
 * may still be good; one that needs a new pairing goes straight to it.
 */
@Composable
internal fun FatalScreen(
    fatal: Link.Fatal,
    /** Why forgetting the host failed, if it did: a connection's error, never an operation's. */
    connectError: String?,
    retry: () -> Unit,
    forget: () -> Unit,
) {
    val hostKeyMismatch = fatal.reason == FatalReason.HOST_KEY_MISMATCH
    var confirmingForget by rememberSaveable { mutableStateOf(false) }
    StandalonePage {
        Text(
            if (hostKeyMismatch) "Machine identity changed" else "Host connection failed",
            modifier = Modifier.semantics { heading() },
            style = MaterialTheme.typography.headlineMedium,
        )
        Spacer(Modifier.height(12.dp))
        Text(
            if (hostKeyMismatch) {
                "The host certificate no longer matches this pairing. Create a fresh pairing code on the host, then pair again."
            } else {
                fatal.message
            },
        )
        connectError?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, modifier = Modifier.announced(), color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(24.dp))
        if (fatal.retryable) {
            Button(onClick = retry) { Text("Try again") }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = { confirmingForget = true }) { Text(FORGET_HOST_TITLE) }
        } else {
            Button(onClick = forget) { Text(FORGET_HOST_TITLE) }
        }
    }
    if (confirmingForget) {
        ForgetHostDialog(
            onForget = {
                confirmingForget = false
                forget()
            },
            onDismiss = { confirmingForget = false },
        )
    }
}

/** Asks before erasing the pairing: getting it back takes a new code from the host. */
@Composable
internal fun ForgetHostDialog(onForget: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Forget this host?") },
        text = { Text("This device will need a new pairing code to connect again.") },
        confirmButton = {
            TextButton(onClick = onForget) { Text("Forget", color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
