package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link

@Composable
internal fun PairScreen(model: MainViewModel) {
    var host by rememberSaveable { mutableStateOf("") }
    var port by rememberSaveable { mutableStateOf("4001") }
    var code by rememberSaveable { mutableStateOf("") }
    val pairing by model.pairing.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    val valid = host.isNotBlank() &&
        port.toIntOrNull() in 1..65535 &&
        code.length == 9

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(24.dp)
            .imePadding(),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Pair with xd", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "Run xd serve --pair on the machine, then enter its address and code. " +
                "This app supplies its model as the device name; the owner can " +
                "rename it later."
        )
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = host,
            onValueChange = { host = it.trim() },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Host or Tailscale IP") },
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = port,
            onValueChange = { port = it.filter(Char::isDigit).take(5) },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Port") },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = code,
            onValueChange = { code = formatPairingCode(it) },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Pairing code") },
            supportingText = { Text(PAIRING_ALPHABET) },
            singleLine = true,
        )
        error?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(20.dp))
        Button(
            onClick = {
                model.pair(host, port.toInt(), code)
            },
            enabled = valid && !pairing,
            modifier = Modifier.fillMaxWidth(),
        ) {
            if (pairing) {
                CircularProgressIndicator(
                    modifier = Modifier.width(20.dp),
                    strokeWidth = 2.dp,
                )
            } else {
                Text("Pair")
            }
        }
    }
}

/**
 * A pinned certificate that no longer matches, or a token the daemon no longer
 * knows, is terminal. There is deliberately no trust-anyway action: the token
 * is remote code execution on the daemon machine.
 */
@Composable
internal fun FatalScreen(
    fatal: Link.Fatal,
    operationError: String?,
    forget: () -> Unit,
) {
    val pinMismatch = fatal.reason == FatalReason.PIN_MISMATCH
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            if (pinMismatch) "Machine identity changed" else "Connection refused",
            style = MaterialTheme.typography.headlineMedium,
        )
        Spacer(Modifier.height(12.dp))
        Text(
            if (pinMismatch) {
                "This is not the machine you paired with: its certificate changed. " +
                    "Re-pair only if you changed or reinstalled the daemon yourself."
            } else {
                fatal.message
            },
        )
        operationError?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(24.dp))
        Button(onClick = forget) { Text("Forget and re-pair") }
    }
}

/** Mirrors the daemon's `XXXX-XXXX` code, whose alphabet omits I, O, 0 and 1. */
private fun formatPairingCode(input: String): String {
    val raw = input.uppercase().filter { it in PAIRING_ALPHABET }.take(8)
    return if (raw.length > 4) raw.take(4) + "-" + raw.drop(4) else raw
}

private const val PAIRING_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
