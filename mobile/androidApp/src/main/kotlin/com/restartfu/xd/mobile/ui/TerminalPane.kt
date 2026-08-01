package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.terminal.Cell

/**
 * The xterm 16-colour palette, so SGR codes land on recognisable colours.
 */
private val PALETTE = listOf(
    Color(0xFF000000), Color(0xFFC01C28), Color(0xFF26A269), Color(0xFFA2734C),
    Color(0xFF12488B), Color(0xFFA347BA), Color(0xFF2AA1B3), Color(0xFFD0CFCC),
    Color(0xFF5E5C64), Color(0xFFF66151), Color(0xFF33D17A), Color(0xFFE9AD0C),
    Color(0xFF2A7BDE), Color(0xFFC061CB), Color(0xFF33C7DE), Color(0xFFFFFFFF),
)

@Composable
internal fun TerminalPaneContent(model: TerminalViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    var input by remember { mutableStateOf("") }
    val fallback = MaterialTheme.colorScheme.onSurface

    Column(Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                if (state.closed) "Session closed" else "Shared session",
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
            if (state.closed || state.error != null) {
                TextButton(onClick = model::open) { Text("Reopen") }
            } else {
                TextButton(onClick = model::kill) { Text("Kill") }
            }
        }

        when {
            state.connecting -> Centered { CircularProgressIndicator() }
            state.error != null -> Centered {
                Text(
                    state.error.orEmpty(),
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(16.dp),
                )
            }
            else -> Column(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .background(Color(0xFF12100E))
                    .verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState())
                    .padding(6.dp),
            ) {
                state.rows.forEach { row ->
                    Text(
                        remember(row) { row.render(fallback) },
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodySmall,
                        softWrap = false,
                    )
                }
            }
        }

        // A phone has no key events to forward, so a line is submitted at a
        // time. Enter sends, which is what a shell expects.
        OutlinedTextField(
            value = input,
            onValueChange = { input = it },
            modifier = Modifier
                .fillMaxWidth()
                .imePadding()
                .padding(8.dp),
            label = { Text("Send a line") },
            enabled = state.id != null && !state.closed,
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
            keyboardActions = KeyboardActions(
                onSend = {
                    model.send(input + "\n")
                    input = ""
                },
            ),
        )
    }
}

private fun List<Cell>.render(fallback: Color): AnnotatedString = buildAnnotatedString {
    // Trailing blanks carry no styling worth drawing and make every row as
    // wide as the screen.
    val line = dropLastWhile { it.char == ' ' && it.background == null }
    line.forEach { cell ->
        val foreground = cell.foreground?.let(::colour) ?: fallback
        val background = cell.background?.let(::colour) ?: Color.Unspecified
        withStyle(
            SpanStyle(
                color = if (cell.inverse) background.takeOrElse(fallback) else foreground,
                background = if (cell.inverse) foreground else background,
                fontWeight = if (cell.bold) FontWeight.Bold else null,
            ),
        ) {
            append(cell.char)
        }
    }
}

private fun colour(index: Int): Color = PALETTE.getOrElse(index) { PALETTE[7] }

private fun Color.takeOrElse(other: Color): Color =
    if (this == Color.Unspecified) other else this
