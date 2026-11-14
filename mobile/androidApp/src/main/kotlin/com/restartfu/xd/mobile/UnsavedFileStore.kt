package com.restartfu.xd.mobile

import android.os.Bundle
import androidx.lifecycle.SavedStateHandle
import com.restartfu.xd.files.OpenFile

/**
 * Unsaved file edits by session, kept in the activity's saved state, so they
 * outlive Android ending the app while it is in the background and come back
 * still unsaved. For each file it keeps the path, what is on screen, and what
 * the host last handed over, which a save is still checked against.
 *
 * Android holds saved state in a buffer the whole app shares, so this keeps
 * at most [limitBytes]; an edit past that is not kept, and the Files pane
 * says so.
 */
internal class UnsavedFileStore(
    handle: SavedStateHandle,
    private val limitBytes: Long = UNSAVED_LIMIT_BYTES,
) {
    private val kept = LinkedHashMap<String, List<OpenFile>>()

    init {
        handle.get<Bundle>(KEY)?.let { saved ->
            for (session in saved.keySet()) {
                val files = saved.getStringArrayList(session).orEmpty().chunked(3)
                    .filter { it.size == 3 }
                    .map { (path, text, base) -> OpenFile(path = path, saved = base, text = text) }
                    .filter { it.dirty }
                if (files.isNotEmpty()) kept[session] = files
            }
        }
        // Written only when Android saves the activity, not on every keystroke.
        handle.setSavedStateProvider(KEY) {
            Bundle().apply {
                kept.forEach { (session, files) ->
                    putStringArrayList(session, ArrayList(files.flatMap { listOf(it.path, it.text, it.saved) }))
                }
            }
        }
    }

    /** The store for one session's Files pane. */
    fun session(id: String): com.restartfu.xd.mobile.UnsavedFiles = object : com.restartfu.xd.mobile.UnsavedFiles {
        override fun restore(): List<OpenFile> = kept[id].orEmpty()

        override fun keep(files: List<OpenFile>): Set<String> {
            var used = kept.filterKeys { it != id }.values.sumOf { list -> list.sumOf(::bytesOf) }
            val keeping = mutableListOf<OpenFile>()
            val tooLarge = mutableSetOf<String>()
            for (file in files) {
                val size = bytesOf(file)
                if (used + size <= limitBytes) {
                    keeping += file
                    used += size
                } else {
                    tooLarge += file.path
                }
            }
            if (keeping.isEmpty()) kept.remove(id) else kept[id] = keeping
            return tooLarge
        }
    }

    /** Lets go of the edits of sessions that are gone. */
    fun retain(sessionIds: Set<String>) {
        kept.keys.retainAll(sessionIds)
    }

    /** Lets go of everything, as forgetting the host does. */
    fun clear() = kept.clear()

    private companion object {
        const val KEY = "unsaved-files"
    }
}

/** How much unsaved text is kept through the end of the process, across every session. */
internal const val UNSAVED_LIMIT_BYTES = 512L * 1024

/** What a file's edit takes in saved state: its path and both texts, two bytes a character. */
private fun bytesOf(file: OpenFile): Long = 2L * (file.path.length + file.text.length + file.saved.length)
