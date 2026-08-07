package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.IntSize
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.terminal.Cell
import com.restartfu.xd.terminal.TerminalKey

/** The xterm 16-colour palette, so SGR codes land on recognisable colours. */
private val PALETTE = listOf(
    // The desktop terminal palette, colour for colour.
    Color(0xFF23232A), Color(0xFFE06C75), Color(0xFF98C379), Color(0xFFD19A66),
    Color(0xFF61AFEF), Color(0xFFC678DD), Color(0xFF56B6C2), Color(0xFFB8BCC8),
    Color(0xFF5C6370), Color(0xFFE06C75), Color(0xFF98C379), Color(0xFFD19A66),
    Color(0xFF61AFEF), Color(0xFFC678DD), Color(0xFF56B6C2), Color(0xFFE6E6EC),
)

// .xd-terminal on the desktop.
private val BACKGROUND = Color(0xFF0A0A0C)
private val FOREGROUND = Color(0xFFF2F2F4)

/**
 * Keys a soft keyboard has no way to send, and a shell cannot do without.
 * Ctrl is sticky: it applies to the next character typed.
 */
private val KEY_BAR = listOf(
    "esc" to TerminalKey.ESCAPE,
    "tab" to TerminalKey.TAB,
    "↑" to TerminalKey.UP,
    "↓" to TerminalKey.DOWN,
    "←" to TerminalKey.LEFT,
    "→" to TerminalKey.RIGHT,
)

@Composable
internal fun TerminalPaneContent(model: TerminalViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val focus = remember { FocusRequester() }
    var control by remember { mutableStateOf(false) }

    // The IME is driven through a field the reader never sees. Android
    // delivers typed characters as text edits rather than key events, so
    // capturing them any other way means missing most of the keyboard. The
    // padding keeps the field non-empty, which is the only way a backspace on
    // an already-empty field is observable at all.
    var buffer by remember { mutableStateOf(TextFieldValue(PAD, TextRange(PAD.length))) }

    Column(
        Modifier
            .fillMaxSize()
            .background(BACKGROUND),
    ) {
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
            else -> BoxWithConstraints(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    // Tapping the screen focuses it, which raises the
                    // keyboard, exactly as tapping a terminal window does.
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                    ) { focus.requestFocus() },
            ) {
                // The pty is told what fits here, so its lines break where the
                // screen breaks them and its last row is the last row shown.
                // Measured from the font rather than guessed: the app scales
                // density, and the reader may have asked for larger text.
                val cell = measureCell()
                val gutter = padding(LocalDensity.current)
                val columns = (constraints.maxWidth - gutter) / cell.width
                val rows = (constraints.maxHeight - gutter) / cell.height
                LaunchedEffect(columns, rows) { model.resize(columns, rows) }

                Column(
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .horizontalScroll(rememberScrollState())
                        .padding(6.dp),
                ) {
                    state.rows.forEachIndexed { index, row ->
                        val caret = if (index == state.cursorRow) state.cursorColumn else -1
                        Text(
                            remember(row, caret) { row.render(caret) },
                            fontFamily = FontFamily.Monospace,
                            style = MaterialTheme.typography.bodySmall,
                            color = FOREGROUND,
                            softWrap = false,
                        )
                    }
                }

                BasicTextField(
                    value = buffer,
                    onValueChange = { next ->
                        typed(next.text, model, control)?.let { control = false }
                        buffer = TextFieldValue(PAD, TextRange(PAD.length))
                    },
                    modifier = Modifier
                        .size(1.dp)
                        .focusRequester(focus)
                        .onKeyEvent { event ->
                            if (event.type != KeyEventType.KeyDown) return@onKeyEvent false
                            val key = when (event.key) {
                                Key.Enter, Key.NumPadEnter -> TerminalKey.ENTER
                                Key.Backspace -> TerminalKey.BACKSPACE
                                Key.Tab -> TerminalKey.TAB
                                Key.Escape -> TerminalKey.ESCAPE
                                Key.DirectionUp -> TerminalKey.UP
                                Key.DirectionDown -> TerminalKey.DOWN
                                Key.DirectionLeft -> TerminalKey.LEFT
                                Key.DirectionRight -> TerminalKey.RIGHT
                                else -> null
                            }
                            key?.let { model.press(it) } != null
                        },
                    keyboardOptions = KeyboardOptions(
                        autoCorrectEnabled = false,
                        capitalization = KeyboardCapitalization.None,
                        imeAction = ImeAction.None,
                    ),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(Color.Transparent),
                )
            }
        }

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState())
                .imePadding()
                .padding(horizontal = 8.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FilterChip(
                selected = control,
                onClick = { control = !control },
                label = { Text("ctrl") },
                enabled = state.id != null && !state.closed,
            )
            KEY_BAR.forEach { (label, key) ->
                FilterChip(
                    selected = false,
                    onClick = { model.press(key) },
                    label = { Text(label) },
                    enabled = state.id != null && !state.closed,
                )
            }
        }
    }
}

/** One monospace cell of the style the rows are drawn in, in pixels. */
@Composable
private fun measureCell(): IntSize {
    val measurer = rememberTextMeasurer()
    val style = MaterialTheme.typography.bodySmall.copy(
        fontFamily = FontFamily.Monospace,
    )
    return remember(measurer, style, LocalDensity.current) {
        // A row of them, divided back down: one glyph on its own rounds its
        // advance, and eighty of those is a column and a half of drift.
        val sample = "M".repeat(CELL_SAMPLE)
        val measured = measurer.measure(AnnotatedString(sample), style = style)
        // Rounded up, so a fraction of a pixel per cell cannot add up to a
        // column that does not fit.
        IntSize(
            ((measured.size.width + CELL_SAMPLE - 1) / CELL_SAMPLE).coerceAtLeast(1),
            measured.size.height.coerceAtLeast(1),
        )
    }
}

// The 6dp the rows are padded by, on both sides.
private fun padding(density: Density): Int =
    (PADDING_DP * 2 * density.density).toInt()

private const val CELL_SAMPLE = 40
private const val PADDING_DP = 6

/**
 * Turns an IME edit into pty input.
 *
 * Text longer than the padding is what was typed; shorter means a backspace
 * ate into it. Returns null when nothing was sent, so a sticky Ctrl survives
 * until it is actually used.
 */
private fun typed(
    text: String,
    model: TerminalViewModel,
    control: Boolean,
): Unit? = when {
    text.length > PAD.length -> {
        val added = text.substring(PAD.length)
        if (control) {
            added.firstOrNull()?.let(model::control)
        } else {
            model.type(added)
        }
        Unit
    }
    text.length < PAD.length -> {
        model.press(TerminalKey.BACKSPACE)
        Unit
    }
    else -> null
}

private fun List<Cell>.render(caret: Int): AnnotatedString = buildAnnotatedString {
    val trailing = indexOfLast { it.char != ' ' || it.background != null } + 1
    val visible = maxOf(trailing, if (caret >= 0) caret + 1 else 0)
    for (column in 0 until visible) {
        val cell = getOrNull(column) ?: Cell()
        val foreground = cell.foreground?.let(::colour) ?: FOREGROUND
        val background = cell.background?.let(::colour) ?: Color.Unspecified
        // The caret is drawn as a block, the way a terminal draws it.
        val inverse = cell.inverse != (column == caret)
        withStyle(
            SpanStyle(
                color = if (inverse) background.orElse(BACKGROUND) else foreground,
                background = if (inverse) foreground else background,
                fontWeight = if (cell.bold) FontWeight.Bold else null,
            ),
        ) {
            append(cell.char)
        }
    }
}

private fun colour(index: Int): Color = PALETTE.getOrElse(index) { PALETTE[7] }

private fun Color.orElse(other: Color): Color =
    if (this == Color.Unspecified) other else this

private const val PAD = "\u200B\u200B\u200B\u200B"
