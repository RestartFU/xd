package com.restartfu.xd.mobile.ui

import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.Role
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.text
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

/**
 * Keys a soft keyboard has no way to send, and a shell cannot do without,
 * each with the name a screen reader says for it.
 * Ctrl is sticky: it applies to the next character typed.
 */
private val KEY_BAR = listOf(
    Triple("esc", "Escape", TerminalKey.ESCAPE),
    Triple("tab", "Tab", TerminalKey.TAB),
    Triple("↑", "Up", TerminalKey.UP),
    Triple("↓", "Down", TerminalKey.DOWN),
    Triple("←", "Left", TerminalKey.LEFT),
    Triple("→", "Right", TerminalKey.RIGHT),
)

/** The terminal's tap surface, its screen text, and the line it announces, for tests. */
internal const val TERMINAL_SURFACE_TAG = "terminal-surface"
internal const val TERMINAL_SCREEN_TAG = "terminal-screen"
internal const val TERMINAL_LIVE_TAG = "terminal-live"

/** How often, at most, a screen reader is told the line the cursor is on. */
private const val TERMINAL_ANNOUNCE_MILLIS = 1_500L

@Composable
internal fun TerminalPaneContent(
    model: TerminalViewModel,
    showSessionBar: Boolean = true,
) {
    val state by model.state.collectAsStateWithLifecycle()
    val focus = remember { FocusRequester() }
    var control by remember { mutableStateOf(false) }
    // Saved, so turning the phone keeps the question up.
    var confirmingStop by rememberSaveable { mutableStateOf(false) }
    if (confirmingStop) {
        StopTerminalDialog(
            onStop = {
                confirmingStop = false
                model.kill()
            },
            onDismiss = { confirmingStop = false },
        )
    }
    val terminalBackground = MaterialTheme.colorScheme.surface
    val terminalForeground = MaterialTheme.colorScheme.onSurface
    // The theme's sixteen ANSI colours: Dark's pastels would wash out on Light's ground.
    val palette = LocalXdTerminalPalette.current
    val light = LocalXdTheme.current.isLight

    // The IME is driven through a field the reader never sees. Android
    // delivers typed characters as text edits rather than key events, so
    // capturing them any other way means missing most of the keyboard. The
    // padding keeps the field non-empty, which is the only way a backspace on
    // an already-empty field is observable at all.
    var buffer by remember { mutableStateOf(TextFieldValue(PAD, TextRange(PAD.length))) }

    Column(
        Modifier
            .fillMaxSize()
            .background(terminalBackground),
    ) {
        if (showSessionBar) Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                if (state.closed) "Terminal closed" else "Shared terminal",
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            // A terminal that failed to open says Try again under its error instead.
            if (state.closed) {
                TextButton(onClick = model::open) { Text("Reopen") }
            } else if (state.error == null) {
                // The shell is shared: Stop asks first, since it ends it everywhere.
                TextButton(
                    onClick = { confirmingStop = true },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) { Text("Stop") }
            }
        }

        when {
            state.connecting -> Centered { CircularProgressIndicator() }
            state.error != null -> Box(Modifier.weight(1f)) { PaneError(state.error.orEmpty(), retry = model::open) }
            else -> BoxWithConstraints(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .testTag(TERMINAL_SURFACE_TAG)
                    .semantics { contentDescription = "Terminal" }
                    // Tapping the screen focuses it, which raises the
                    // keyboard, exactly as tapping a terminal window does.
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClickLabel = "Show keyboard",
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

                // What is on the screen, as one piece of text a screen reader
                // reads through, rather than a stop for every row.
                val screenText = remember(state.rows) { state.rows.screenText() }
                Column(
                    Modifier
                        .fillMaxSize()
                        .testTag(TERMINAL_SCREEN_TAG)
                        .semanticsNode { text = AnnotatedString(screenText.ifEmpty { "No output yet" }) }
                        .verticalScroll(rememberScrollState())
                        .horizontalScroll(rememberScrollState())
                        .padding(6.dp),
                ) {
                    state.rows.forEachIndexed { index, row ->
                        val caret = if (index == state.cursorRow) state.cursorColumn else -1
                        Text(
                            remember(row, caret, terminalForeground, terminalBackground, palette, light) {
                                row.render(caret, terminalForeground, terminalBackground, palette, light)
                            },
                            fontFamily = XdMonoFamily,
                            style = MaterialTheme.typography.bodySmall,
                            color = terminalForeground,
                            softWrap = false,
                        )
                    }
                }

                // New output is announced as the line the cursor is on, at a
                // pace: a build can print hundreds of lines a second.
                val cursorLine = state.rows.getOrNull(state.cursorRow)?.lineText().orEmpty()
                val announced = rememberThrottled(cursorLine, TERMINAL_ANNOUNCE_MILLIS)
                Box(
                    Modifier
                        .size(1.dp)
                        .testTag(TERMINAL_LIVE_TAG)
                        .semanticsNode {
                            liveRegion = LiveRegionMode.Polite
                            contentDescription = announced
                        },
                )

                BasicTextField(
                    value = buffer,
                    onValueChange = { next ->
                        typed(next.text, model, control)?.let { control = false }
                        buffer = TextFieldValue(PAD, TextRange(PAD.length))
                    },
                    modifier = Modifier
                        .size(1.dp)
                        .focusRequester(focus)
                        // The field holds only padding, so it is named for what typing into it does.
                        .semantics { contentDescription = "Terminal input" }
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
            val live = state.id != null && !state.closed
            // Ctrl is the one key that stays down until the next: a switch that says whether it is on.
            FilterChip(
                selected = control,
                onClick = { control = !control },
                label = { Text("Ctrl") },
                enabled = live,
                modifier = Modifier.semantics {
                    contentDescription = "Control, for the next key"
                    role = Role.Switch
                    toggleableState = ToggleableState(control)
                },
            )
            // The rest are pressed and let go: buttons, not choices.
            KEY_BAR.forEach { (label, name, key) ->
                AssistChip(
                    onClick = { model.press(key) },
                    label = { Text(label) },
                    enabled = live,
                    modifier = Modifier.semantics { contentDescription = name },
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
        fontFamily = XdMonoFamily,
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

/** A row's characters, without the blank cells after the last one. */
private fun List<Cell>.lineText(): String = buildString { this@lineText.forEach { append(it.char) } }.trimEnd()

/** Every row with something on it, top to bottom. */
private fun List<List<Cell>>.screenText(): String =
    map { it.lineText() }.dropLastWhile(String::isEmpty).joinToString("\n")

private fun List<Cell>.render(
    caret: Int,
    defaultForeground: Color,
    defaultBackground: Color,
    palette: List<Color>,
    light: Boolean,
): AnnotatedString = buildAnnotatedString {
    val trailing = indexOfLast { it.char != ' ' || it.background != null } + 1
    val visible = maxOf(trailing, if (caret >= 0) caret + 1 else 0)
    // One span for each run of cells drawn alike, not one per character:
    // a row of plain output is a single span.
    var run: SpanStyle? = null
    val text = StringBuilder()
    fun flush() {
        val style = run ?: return
        withStyle(style) { append(text.toString()) }
        text.setLength(0)
    }
    for (column in 0 until visible) {
        val cell = getOrNull(column) ?: Cell()
        val (ink, ground) = cell.colours(column == caret, defaultForeground, defaultBackground, palette, light)
        val style = SpanStyle(
            color = ink,
            background = ground ?: Color.Unspecified,
            fontWeight = if (cell.bold) FontWeight.Bold else null,
        )
        if (style != run) {
            flush()
            run = style
        }
        text.append(cell.char)
    }
    flush()
}

/** Asks before the shared shell is stopped, here and on every other device. */
@Composable
internal fun StopTerminalDialog(onStop: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Stop this terminal?") },
        text = { Text("It closes the shell on every device, and anything running in it.") },
        confirmButton = {
            TextButton(onClick = onStop) { Text("Stop", color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * The text colour and the background, if it has one of its own, that a cell
 * is drawn in. The caret is drawn as a block, the way a terminal draws it:
 * the cell under it swaps its two colours, as an inverse cell does. What
 * comes out is then kept readable by the desktop's rule, [terminalCellInk].
 */
internal fun Cell.colours(
    caret: Boolean,
    defaultForeground: Color,
    defaultBackground: Color,
    palette: List<Color>,
    light: Boolean,
): Pair<Color, Color?> {
    val foreground = foreground?.let { palette.colour(it) }
    val background = background?.let { palette.colour(it) }
    val swapped = inverse != caret
    val ink = if (swapped) background ?: defaultBackground else foreground
    val ground = if (swapped) foreground ?: defaultForeground else background
    return (terminalCellInk(ink, ground, palette, light) ?: defaultForeground) to ground
}

private fun List<Color>.colour(index: Int): Color = getOrElse(index) { this[7] }

private const val PAD = "\u200B\u200B\u200B\u200B"
