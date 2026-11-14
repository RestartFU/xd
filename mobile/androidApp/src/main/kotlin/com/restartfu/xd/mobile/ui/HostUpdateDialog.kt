package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.delay
import com.restartfu.xd.mobile.MainViewModel

/**
 * Updates the machine this device is paired with.
 *
 * Install and restart are separate buttons on purpose. Replacing the files is
 * safe while turns run; restarting drops every attached device and loses
 * whatever the agent was doing, so it is never automatic.
 */
@Composable
internal fun HostUpdateDialog(
    model: MainViewModel,
    onDismiss: () -> Unit,
) {
    val status by model.host.collectAsStateWithLifecycle()
    val error by model.hostError.collectAsStateWithLifecycle()
    val busy by model.hostBusy.collectAsStateWithLifecycle()
    // Until the host confirms the check, an earlier result is not shown as current.
    val checkPending by model.hostCheckPending.collectAsStateWithLifecycle()

    LaunchedEffect(Unit) { model.hostUpdate("check") }
    // Checking and installing finish on the host after it answers, so ask
    // again until they do. A failed request is asked about again too: the
    // host may have taken an install whose answer was lost.
    val pending = status?.state == "checking" || status?.state == "installing" || error != null
    LaunchedEffect(pending) {
        while (pending) {
            delay(HOST_UPDATE_POLL_MILLIS)
            model.followHostUpdate()
        }
    }

    val supported = status?.supported == true
    val state = status?.state ?: "idle"
    val installable = supported && !busy &&
        (status?.available == true || state == "failed")
    val restartable = supported && !busy && state == "installed"

    AlertDialog(
        onDismissRequest = {
            model.clearHostUpdate()
            onDismiss()
        },
        title = { Text("Update host") },
        text = {
            Column {
                Text(
                    when {
                        error != null -> error.orEmpty()
                        status == null || state == "checking" || checkPending ->
                            "Checking for an update…"
                        !supported ->
                            "This machine's installation cannot update itself. " +
                                "Update it the way it was installed."
                        state == "installing" ->
                            "Installing. The host keeps running until restarted."
                        state == "installed" ->
                            "Installed. Restart to run the new build."
                        state == "failed" ->
                            status?.error ?: "The update failed."
                        status?.available == true -> "An update is available."
                        else -> "This machine is up to date."
                    },
                    // It moves on by itself (checking, installing, failed), so each step is said.
                    modifier = Modifier.announced(),
                )
                status?.let { current ->
                    Spacer(Modifier.height(8.dp))
                    Text(
                        buildString {
                            append("Running ").append(current.version)
                            current.latest?.let { append(" · latest ").append(it) }
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (restartable) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Restarting drops every attached device and loses any " +
                            "running turn.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        },
        confirmButton = {
            Row {
                if (restartable) {
                    TextButton(onClick = { model.hostUpdate("restart") }) {
                        Text("Restart")
                    }
                }
                TextButton(
                    onClick = { model.hostUpdate("install") },
                    enabled = installable,
                ) {
                    Text("Install")
                }
            }
        },
        dismissButton = {
            TextButton(
                onClick = {
                    model.clearHostUpdate()
                    onDismiss()
                },
            ) { Text("Close") }
        },
    )
}

private const val HOST_UPDATE_POLL_MILLIS = 2_000L
