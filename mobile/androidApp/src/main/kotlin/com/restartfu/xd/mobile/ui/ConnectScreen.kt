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
    var host by rememberSaveable { mutableStateOf("") }
    var port by rememberSaveable { mutableStateOf("22") }
    var username by rememberSaveable { mutableStateOf("") }
    var usePrivateKey by rememberSaveable { mutableStateOf(false) }
    var password by remember { mutableStateOf("") }
    var passphrase by remember { mutableStateOf("") }
    val connecting by model.connecting.collectAsStateWithLifecycle()
    val pendingHostKey by model.pendingHostKey.collectAsStateWithLifecycle()
    val privateKeyName by model.privateKeyName.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    val keyPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null && pendingHostKey == null) model.importPrivateKey(uri, uri.lastPathSegment)
    }
    val inputEnabled = pendingHostKey == null && !connecting

    val valid = host.isNotBlank() &&
        port.toIntOrNull() in 1..65535 &&
        username.isNotBlank() &&
        if (usePrivateKey) privateKeyName != null else password.isNotEmpty()

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(24.dp)
            .imePadding(),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Connect to xd over SSH", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "Use the same SSH account as the desktop app. The remote machine must already " +
                "have the current xd host installed."
        )
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = host,
            onValueChange = { host = it.trim() },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Host or Tailscale IP") },
            enabled = inputEnabled,
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = port,
            onValueChange = { port = it.filter(Char::isDigit).take(5) },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("SSH port") },
            enabled = inputEnabled,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("SSH username") },
            enabled = inputEnabled,
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(
                selected = !usePrivateKey,
                onClick = { usePrivateKey = false },
                enabled = inputEnabled,
                label = { Text("Password") },
            )
            FilterChip(
                selected = usePrivateKey,
                onClick = { usePrivateKey = true },
                enabled = inputEnabled,
                label = { Text("Private key") },
            )
        }
        Spacer(Modifier.height(12.dp))
        if (usePrivateKey) {
            OutlinedButton(
                onClick = { keyPicker.launch(arrayOf("*/*")) },
                enabled = inputEnabled,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(privateKeyName ?: "Choose private key")
            }
            if (privateKeyName != null) {
                Spacer(Modifier.height(8.dp))
                OutlinedButton(
                    onClick = model::clearPrivateKey,
                    enabled = inputEnabled,
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Remove private key") }
            }
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = passphrase,
                onValueChange = { passphrase = it },
                modifier = Modifier.fillMaxWidth(),
                label = { Text("Private-key passphrase, if any") },
                enabled = inputEnabled,
                visualTransformation = PasswordVisualTransformation(),
                singleLine = true,
            )
        } else {
            OutlinedTextField(
                value = password,
                onValueChange = { password = it },
                modifier = Modifier.fillMaxWidth(),
                label = { Text("SSH password") },
                enabled = inputEnabled,
                visualTransformation = PasswordVisualTransformation(),
                singleLine = true,
            )
        }
        error?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        pendingHostKey?.let { confirmation ->
            Spacer(Modifier.height(20.dp))
            Text("Verify the SSH host key", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(8.dp))
            Text("Trust host ${confirmation.host}:${confirmation.port} as ${confirmation.username}")
            Spacer(Modifier.height(8.dp))
            Text("${confirmation.hostKey.algorithm}  ${confirmation.hostKey.fingerprint}")
            Spacer(Modifier.height(8.dp))
            Text("Compare this fingerprint with ssh-keygen or your administrator before trusting it.")
            Spacer(Modifier.height(12.dp))
            Button(
                onClick = model::confirmHostKey,
                enabled = !connecting,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Trust and connect") }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(
                onClick = model::cancelHostKeyConfirmation,
                enabled = !connecting,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Cancel") }
        } ?: run {
            Spacer(Modifier.height(20.dp))
            Button(
                onClick = {
                    model.connect(
                        host = host,
                        port = port.toInt(),
                        username = username,
                        password = password,
                        usePrivateKey = usePrivateKey,
                        passphrase = passphrase,
                    )
                },
                enabled = valid && !connecting,
                modifier = Modifier.fillMaxWidth(),
            ) {
                if (connecting) {
                    CircularProgressIndicator(
                        modifier = Modifier.width(20.dp),
                        strokeWidth = 2.dp,
                    )
                } else {
                    Text("Connect")
                }
            }
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
            if (hostKeyMismatch) "Machine identity changed" else "SSH connection failed",
            style = MaterialTheme.typography.headlineMedium,
        )
        Spacer(Modifier.height(12.dp))
        Text(
            if (hostKeyMismatch) {
                "The machine's SSH host key no longer matches the pinned key. Forget it only " +
                    "after independently verifying the new fingerprint."
            } else {
                fatal.message
            },
        )
        operationError?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(24.dp))
        Button(onClick = forget) { Text("Forget SSH connection") }
    }
}
