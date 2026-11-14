package com.restartfu.xd.mobile.ui

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import java.util.concurrent.atomic.AtomicLong

/**
 * The file editor's text as typed, with how many line breaks it holds and a
 * revision that names this text, both kept from each edit rather than worked
 * out again from the whole text on every keystroke: a keystroke counts the
 * breaks only in what it typed or deleted, and the colour cache is keyed by
 * the revision (see [EditorKey]) instead of a hash of the whole file.
 */
@Stable
internal class EditorText(text: String) {
    var value: TextFieldValue by mutableStateOf(TextFieldValue(text))
        private set

    /** How many '\n' the text holds. */
    var newlines: Int = countNewlines(text, 0, text.length)
        private set

    /** Names this text among every text any editor has held in this process. */
    var revision: Long = EditorRevisions.incrementAndGet()
        private set

    /** Has been typed into since it was opened. */
    var edited: Boolean = false
        private set

    /**
     * Takes [text] from outside (the host's copy, a save, a discard), unless it
     * is the very text typed here, which the Files pane hands back as it is.
     */
    fun sync(text: String) {
        if (text === value.text) return
        val selection = TextRange(value.selection.start.coerceAtMost(text.length), value.selection.end.coerceAtMost(text.length))
        value = TextFieldValue(text, selection)
        newlines = countNewlines(text, 0, text.length)
        revision = EditorRevisions.incrementAndGet()
    }

    /**
     * One change from the field. Returns the new text when the text changed,
     * or null when only the caret, the selection or the word being composed did.
     */
    fun edit(next: TextFieldValue): String? {
        val old = value
        if (next.text.length == old.text.length && next.text == old.text) {
            // The same text, kept as the same string, so the colouring is not worked out again.
            value = next.copy(text = old.text)
            return null
        }
        newlines = newlinesAfterEdit(old, next, newlines)
        value = next
        revision = EditorRevisions.incrementAndGet()
        edited = true
        return next.text
    }
}

private val EditorRevisions = AtomicLong()

/** How many '\n' [text] holds from [start] to [end]. */
internal fun countNewlines(text: CharSequence, start: Int, end: Int): Int {
    var count = 0
    for (index in start until end) if (text[index] == '\n') count++
    return count
}

/**
 * How many line breaks [next] holds, given [before] in [old], from what the
 * edit typed or deleted at the caret: typing or pasting at the caret or over
 * a selection, Backspace and Delete. Any other change, such as the keyboard
 * rewriting a word away from the caret, counts the whole text afresh.
 */
internal fun newlinesAfterEdit(old: TextFieldValue, next: TextFieldValue, before: Int): Int {
    val was = old.text
    val now = next.text
    val start = old.selection.min
    val end = old.selection.max
    val caret = next.selection
    if (caret.collapsed) {
        val removed = end - start
        val inserted = now.length - (was.length - removed)
        // Typed or pasted at the caret, or over the selection: the new text is
        // the old one with [start, end) replaced by what ends at the caret.
        if (inserted > 0 && caret.start == start + inserted) {
            return before - countNewlines(was, start, end) + countNewlines(now, start, start + inserted)
        }
        // A selection deleted.
        if (removed > 0 && inserted == 0 && caret.start == start) {
            return before - countNewlines(was, start, end)
        }
        if (removed == 0 && now.length < was.length) {
            val gone = was.length - now.length
            // Backspace: the caret moves back over what it deleted.
            if (caret.start == start - gone) return before - countNewlines(was, caret.start, start)
            // Delete: the caret stays, and what followed it goes.
            if (caret.start == start && start + gone <= was.length) return before - countNewlines(was, start, start + gone)
        }
    }
    return countNewlines(now, 0, now.length)
}
