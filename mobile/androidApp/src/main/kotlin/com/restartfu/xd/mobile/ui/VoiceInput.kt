package com.restartfu.xd.mobile.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.R
import com.restartfu.xd.voice.VoiceSession
import com.restartfu.xd.voice.VoiceState

/**
 * The microphone, and everything one recording can be doing.
 *
 * Only capture happens here. The daemon holds the speech model and runs
 * whisper, so a phone never downloads 148 MB or spends its battery on
 * transcription — and a remote chat transcribes on the remote machine.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun VoiceButton(voice: VoiceSession) {
    val state by voice.state.collectAsStateWithLifecycle()
    val handsFree by voice.handsFree.collectAsStateWithLifecycle()
    val context = LocalContext.current
    // Which the permission prompt was for. A grant has to resume what was
    // asked for, or a long press that trips the prompt lands as one recording.
    var wantsHandsFree by remember { mutableStateOf(false) }
    val request = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) {
            if (wantsHandsFree) voice.setHandsFree(true) else voice.start()
        } else {
            voice.reportFailure("Microphone access is needed to dictate")
        }
    }

    // Hands-free owns the button for as long as it is on: every state it passes
    // through is one it will leave by itself, and the only thing left to offer
    // is the way out.
    if (handsFree) {
        FilledIconButton(onClick = { voice.setHandsFree(false) }) {
            Icon(
                painter = painterResource(R.drawable.ic_mic),
                contentDescription = "Stop hands-free dictation",
            )
        }
        return
    }

    when (state) {
        is VoiceState.Recording -> Button(onClick = voice::stop) { Text("Stop") }
        // A round trip and a transcription are both short and both
        // uninterruptible-looking; showing progress beats a dead button.
        VoiceState.Checking, VoiceState.Transcribing ->
            TextButton(onClick = voice::cancel) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
            }
        is VoiceState.Downloading, VoiceState.NeedsModel -> Unit
        // Square and the same size as the attach button beside it: they are
        // the same kind of thing and should be the same thing to hit.
        //
        // A long press is hands-free, which is the same gesture the tree uses
        // for the second thing a row can do.
        else -> Box(
            modifier = Modifier
                .size(48.dp)
                .clip(CircleShape)
                .combinedClickable(
                    onClick = {
                        wantsHandsFree = false
                        if (context.canRecord()) {
                            voice.start()
                        } else {
                            request.launch(Manifest.permission.RECORD_AUDIO)
                        }
                    },
                    onLongClick = {
                        wantsHandsFree = true
                        if (context.canRecord()) {
                            voice.setHandsFree(true)
                        } else {
                            request.launch(Manifest.permission.RECORD_AUDIO)
                        }
                    },
                    onClickLabel = "Dictate",
                    onLongClickLabel = "Dictate hands-free",
                ),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painter = painterResource(R.drawable.ic_mic),
                contentDescription = "Dictate",
            )
        }
    }
}

/**
 * What voice input is doing, above the composer.
 *
 * Nothing at all while idle: a microphone that is not recording has no state
 * worth a row of the screen.
 */
@Composable
internal fun VoiceStatus(voice: VoiceSession) {
    val state by voice.state.collectAsStateWithLifecycle()
    val partial by voice.partial.collectAsStateWithLifecycle()
    val handsFree by voice.handsFree.collectAsStateWithLifecycle()

    when (val current = state) {
        is VoiceState.Recording -> Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                when {
                    partial.isNotBlank() -> partial
                    // Nothing said yet, and hands-free has no button coming to
                    // explain itself. Say what it is waiting for.
                    handsFree -> "Hands-free — speak, then pause to send"
                    else -> "Recording ${clock(elapsedSeconds(current.startedAtMillis))}"
                },
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.primary,
            )
            TextButton(onClick = voice::cancel) {
                Text(if (handsFree) "Stop" else "Discard")
            }
        }
        // Between utterances the microphone is shut but the loop is not over,
        // and a row that vanished each time would flicker on every sentence.
        VoiceState.Checking, VoiceState.Transcribing -> if (handsFree) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    if (partial.isBlank()) "Hands-free — listening again…" else partial,
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                TextButton(onClick = voice::cancel) { Text("Stop") }
            }
        } else {
            Unit
        }
        is VoiceState.Downloading -> Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "Speech model ${current.percent}%",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
            LinearProgressIndicator(
                progress = { current.percent / 100f },
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = voice::cancel) { Text("Cancel") }
        }
        is VoiceState.Failed -> Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                current.message,
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
            TextButton(onClick = voice::dismissError) { Text("Dismiss") }
        }
        VoiceState.NeedsModel -> AlertDialog(
            onDismissRequest = voice::cancel,
            title = { Text("Download the speech model?") },
            text = {
                Text(
                    "This machine has no speech model yet. It is a 148 MB " +
                        "download onto the machine running this chat, not onto " +
                        "your phone, and it only happens once.",
                )
            },
            confirmButton = {
                TextButton(onClick = voice::confirmDownload) { Text("Download") }
            },
            dismissButton = {
                TextButton(onClick = voice::cancel) { Text("Not now") }
            },
        )
        else -> Unit
    }
}

/** Elapsed recording time, which reads better as a clock than as seconds. */
private fun clock(seconds: Long): String {
    val safe = seconds.coerceAtLeast(0)
    return "${safe / 60}:${(safe % 60).toString().padStart(2, '0')}"
}

// Context.checkSelfPermission rather than ContextCompat: minSdk is 26, well
// past the API 23 that introduced it, so the compat shim buys nothing.
private fun Context.canRecord(): Boolean =
    checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
        PackageManager.PERMISSION_GRANTED
