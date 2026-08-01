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
import com.restartfu.xd.mobile.MainViewModel

/**
 * Updates the machine this device is paired with.
 *
 * Install and restart are separate buttons on purpose. Replacing the files is
 * safe while turns run; restarting drops every attached device and loses
 * whatever the agent was doing, so it is never automatic.
 */
@Composable
internal fun DaemonUpdateDialog(
    model: MainViewModel,
    onDismiss: () -> Unit,
) {
    val status by model.daemon.collectAsStateWithLifecycle()
    val error by model.daemonError.collectAsStateWithLifecycle()
    val busy by model.daemonBusy.collectAsStateWithLifecycle()

    LaunchedEffect(Unit) { model.daemonUpdate("check") }

    val supported = status?.supported == true
    val state = status?.state ?: "idle"
    val installable = supported && !busy &&
        (status?.available == true || state == "failed")
    val restartable = supported && !busy && state == "installed"

    AlertDialog(
        onDismissRequest = {
            model.clearDaemonUpdate()
            onDismiss()
        },
        title = { Text("Update daemon") },
        text = {
            Column {
                Text(
                    when {
                        error != null -> error.orEmpty()
                        status == null || busy && state == "checking" ->
                            "Checking for an update…"
                        !supported ->
                            "This machine's installation cannot update itself. " +
                                "Update it the way it was installed."
                        state == "installing" ->
                            "Installing. The daemon keeps running until restarted."
                        state == "installed" ->
                            "Installed. Restart to run the new build."
                        state == "failed" ->
                            status?.error ?: "The update failed."
                        status?.available == true -> "An update is available."
                        else -> "This machine is up to date."
                    },
                )
                status?.let { current ->
                    Spacer(Modifier.height(8.dp))
                    Text(
                        buildString {
                            append("Running ").append(current.version)
                            current.latest?.let { append(" · latest ").append(it) }
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
                if (restartable) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Restarting drops every attached device and loses any " +
                            "running turn.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
            }
        },
        confirmButton = {
            Row {
                if (restartable) {
                    TextButton(onClick = { model.daemonUpdate("restart") }) {
                        Text("Restart")
                    }
                }
                TextButton(
                    onClick = { model.daemonUpdate("install") },
                    enabled = installable,
                ) {
                    Text("Install")
                }
            }
        },
        dismissButton = {
            TextButton(
                onClick = {
                    model.clearDaemonUpdate()
                    onDismiss()
                },
            ) { Text("Close") }
        },
    )
}
