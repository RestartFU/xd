package com.restartfu.xd.mobile.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.material3.Button
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link

@Composable
internal fun ConnectScreen(model: MainViewModel) {
    var invitation by remember { mutableStateOf("") }
    val connecting by model.connecting.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp).imePadding(),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Pair with xd", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(12.dp))
        Text("On your host, open Settings → Pair a device. Copy its pairing code here. Both devices need access to the host's LAN or Tailscale address.")
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(value = invitation, onValueChange = { invitation = it }, modifier = Modifier.fillMaxWidth(), label = { Text("Pairing code") }, enabled = !connecting)
        Spacer(Modifier.height(12.dp))
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(onClick = { model.connectPairing(invitation) }, enabled = !connecting && invitation.trim().startsWith("xd1:"), modifier = Modifier.fillMaxWidth()) {
            if (connecting) CircularProgressIndicator() else Text("Pair and connect")
        }
    }
}

@Composable
internal fun FatalScreen(
    fatal: Link.Fatal,
    operationError: String?,
    forget: () -> Unit,
) {
    val hostKeyMismatch = fatal.reason == FatalReason.HOST_KEY_MISMATCH
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            if (hostKeyMismatch) "Machine identity changed" else "Host connection failed",
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
        operationError?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(24.dp))
        Button(onClick = forget) { Text("Forget pairing") }
    }
}
