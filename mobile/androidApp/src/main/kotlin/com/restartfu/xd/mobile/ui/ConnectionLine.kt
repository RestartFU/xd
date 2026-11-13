package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.HorizontalDivider
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.TextButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.ConnectionNotice
import com.restartfu.xd.model.connectionNotice
import com.restartfu.xd.model.secondsUntil
import com.restartfu.xd.net.Link
import kotlinx.coroutines.delay

/** What [ConnectionLine] says on every page; it holds null while the host is connected. */
internal val LocalConnectionNotice = staticCompositionLocalOf<State<ConnectionNotice?>> { mutableStateOf(null) }

/**
 * Makes the next connection attempt at once, in place of the wait before it;
 * null where nothing can (a preview, a test of the line alone).
 */
internal val LocalConnectionRetry = staticCompositionLocalOf<(() -> Unit)?> { null }

/** The action on the line while it waits to try again. */
internal const val TRY_NOW_LABEL = "Try now"

/** The connection line, for tests. */
internal const val CONNECTION_LINE_TAG = "connection-line"

/** How long a first connection may take before the line says so: most finish sooner. */
private const val QUIET_DELAY_MILLIS = 700L

/**
 * What to say about [link], kept in one place for the whole app so every
 * page's line agrees: why the link last went down stays known through the
 * attempts between waits, and the wait counts down by the second. Held as a
 * state, so a second passing redraws the line and nothing else.
 */
@Composable
internal fun rememberConnectionNotice(link: Link): State<ConnectionNotice?> {
    val shown = remember { mutableStateOf<ConnectionNotice?>(null) }
    val failures = remember { LastFailure() }
    LaunchedEffect(link) {
        val lastFailure = failures.after(link)
        when (link) {
            is Link.Down -> {
                var left = secondsUntil(link.nextAttemptInMs)
                while (true) {
                    shown.value = connectionNotice(link, lastFailure, left)
                    if (left <= 0) break
                    delay(1_000)
                    left -= 1
                }
            }
            Link.Idle, Link.Connecting -> {
                val notice = connectionNotice(link, lastFailure)
                if (notice?.failure != true) {
                    // A first connection that comes up within a moment says
                    // nothing at all, as on every return to the app.
                    shown.value = null
                    delay(QUIET_DELAY_MILLIS)
                }
                shown.value = notice
            }
            is Link.Up, is Link.Fatal -> shown.value = null
        }
    }
    return shown
}

/** Why the link last went down, forgotten once it is up again. */
private class LastFailure {
    private var message: String? = null

    fun after(link: Link): String? {
        when (link) {
            is Link.Down -> message = link.message
            is Link.Up, is Link.Fatal -> message = null
            Link.Idle, Link.Connecting -> Unit
        }
        return message
    }
}

/**
 * The one line under a page's header while the host is not connected: quiet
 * while it first connects, and in the error role once the link has failed,
 * with why and when it tries again. While connected it draws nothing, so a
 * header keeps its one row.
 */
@Composable
internal fun ConnectionLine(modifier: Modifier = Modifier, divider: Color = MaterialTheme.colorScheme.outlineVariant) {
    val notice = LocalConnectionNotice.current.value ?: return
    val retry = LocalConnectionRetry.current?.takeIf { notice.waiting }
    val colors = MaterialTheme.colorScheme
    val content = if (notice.failure) colors.onErrorContainer else colors.onSurfaceVariant
    Column(modifier) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(if (notice.failure) colors.errorContainer else glassPane())
                // Slim, and only as tall as its Try now needs while that shows.
                .heightIn(min = 28.dp)
                .padding(start = 16.dp, end = if (retry != null) 4.dp else 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(
                modifier = Modifier
                    .weight(1f)
                    .testTag(CONNECTION_LINE_TAG)
                    // Spoken without the countdown: a second passing is not news.
                    .semanticsNode {
                        liveRegion = LiveRegionMode.Polite
                        contentDescription = notice.spoken
                    }
                    .padding(vertical = 5.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (notice.failure) {
                    Icon(
                        painterResource(R.drawable.ic_alert),
                        contentDescription = null,
                        tint = content,
                        modifier = Modifier.size(14.dp),
                    )
                }
                Text(
                    buildAnnotatedString {
                        withStyle(SpanStyle(fontWeight = FontWeight.Medium)) { append(notice.title) }
                        notice.reason?.let { append(" · $it") }
                    },
                    color = content,
                    style = MaterialTheme.typography.labelMedium,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            retry?.let {
                TextButton(
                    onClick = it,
                    colors = ButtonDefaults.textButtonColors(contentColor = content),
                    contentPadding = PaddingValues(horizontal = 12.dp),
                ) {
                    Text(TRY_NOW_LABEL, style = MaterialTheme.typography.labelLarge)
                }
            }
        }
        HorizontalDivider(color = divider)
    }
}
