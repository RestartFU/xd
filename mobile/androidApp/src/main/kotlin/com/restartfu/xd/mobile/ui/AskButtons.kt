package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.Ask

/**
 * The answers to a tagged question, as buttons.
 *
 * Both bundled assistants run non-interactively, so a question is a block in
 * the reply rather than a prompt. Tapping one sends it as the next message,
 * which is exactly what typing it would do — the buttons only save the typing.
 *
 * One per line rather than wrapped: an option is a whole answer, often a
 * sentence, and a grid of truncated sentences is not a choice anyone can make.
 * A question that only takes typed text gets no buttons; the composer is
 * already there.
 */
@Composable
internal fun AskButtons(ask: Ask, onAnswer: (String) -> Unit, enabled: Boolean) {
    if (ask.options.isEmpty()) return

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        ask.options.forEach { option ->
            OutlinedButton(
                onClick = { onAnswer(option) },
                modifier = Modifier.fillMaxWidth(),
                enabled = enabled,
            ) {
                Text(
                    option,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Start,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}
