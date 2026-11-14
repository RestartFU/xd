package com.restartfu.xd.mobile.ui

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.AgentRelay
import com.restartfu.xd.model.AgentsTray
import com.restartfu.xd.model.DisplayLabels
import com.restartfu.xd.model.SubagentRun
import com.restartfu.xd.model.SubagentState
import com.restartfu.xd.model.compactDuration
import kotlinx.coroutines.delay

/** What an agent tab is called: its task, or its kind when it was given none. */
internal fun agentTitle(run: SubagentRun): String = run.detail.ifBlank { run.identity }

/** Counts up once a second while [ticking], for elapsed times. */
@Composable
private fun rememberNow(ticking: Boolean): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    if (ticking) {
        LaunchedEffect(Unit) {
            while (true) {
                now = System.currentTimeMillis()
                delay(1_000)
            }
        }
    }
    return now
}

/**
 * Every agent still at work, above the composer: background agents keep
 * running after the turn, and the reader may be far from their cards. One
 * line says how many and what they are doing; it opens into a row per agent,
 * and a row opens that agent's tab.
 */
@Composable
internal fun AgentsTray(
    running: List<SubagentRun>,
    open: Boolean,
    onToggle: () -> Unit,
    onOpenAgent: (String) -> Unit,
    compact: Boolean,
    /** The chat's agent chats at work or waiting on the user, from the tree. */
    workers: List<com.restartfu.xd.model.ChatSummary> = emptyList(),
    /** Opens an agent chat by id. */
    onOpenChat: (String) -> Unit = {},
) {
    if (running.isEmpty() && workers.isEmpty()) return
    // Only what waits on the user stops the dot pulsing.
    val allWaiting = running.isEmpty() && workers.all(AgentsTray::needsYou)
    val signals = LocalXdSignals.current
    val now = rememberNow(open)
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = if (compact) 8.dp else 12.dp)
            .padding(top = 6.dp),
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(Modifier.animateContentSize()) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .clickable(
                        role = Role.Button,
                        onClickLabel = if (open) "Hide agents" else "Show agents",
                        onClick = onToggle,
                    )
                    .semantics { stateDescription = if (open) "Expanded" else "Collapsed" }
                    .padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                // The label beside it says which; the shape is the shared mark for each.
                StatusMarkIcon(if (allWaiting) StatusMark.WAITING else StatusMark.WORKING, labelled = false)
                Text(
                    AgentsTray.label(running, workers),
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                )
                Text(
                    if (open) "" else (workers.map { it.title } + running.map { trayTask(it) }).joinToString(", "),
                    modifier = Modifier.weight(1f),
                    color = signals.faint,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Icon(
                    painterResource(if (open) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
                    contentDescription = null,
                    tint = signals.faint,
                    modifier = Modifier.size(14.dp),
                )
            }
            if (open) {
                Column(
                    Modifier
                        .heightIn(max = 240.dp)
                        .verticalScroll(rememberScrollState())
                        .padding(bottom = 4.dp),
                ) {
                    // Agent chats first: each is a chat of its own, which a row opens.
                    workers.forEach { worker ->
                        val needsYou = AgentsTray.needsYou(worker)
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(min = 48.dp)
                                .clickable(onClickLabel = "Open chat") { onOpenChat(worker.id) }
                                .padding(horizontal = 14.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            AgentAvatar(
                                if (needsYou) signals.waiting else signals.working,
                                size = 24.dp,
                                status = if (needsYou) StatusMark.WAITING else StatusMark.WORKING,
                            ) {
                                BackendIcon(worker.backend, size = 12.dp)
                            }
                            Text(
                                worker.title.ifBlank { "New chat" },
                                modifier = Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(
                                if (needsYou) "Needs you" else "Running",
                                color = if (needsYou) signals.waiting else signals.faint,
                                style = MaterialTheme.typography.labelMedium,
                            )
                        }
                    }
                    running.forEach { run ->
                        val key = run.key
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(min = 48.dp)
                                .clickable(enabled = key != null, onClickLabel = "Open agent") {
                                    key?.let(onOpenAgent)
                                }
                                .padding(horizontal = 14.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            AgentAvatar(signals.working, size = 24.dp, status = StatusMark.WORKING) {
                                if (run.isShell) {
                                    Icon(
                                        painterResource(R.drawable.ic_terminal),
                                        contentDescription = null,
                                        tint = signals.faint,
                                        modifier = Modifier.size(12.dp),
                                    )
                                } else {
                                    BackendIcon(run.provider.lowercase(), size = 12.dp)
                                }
                            }
                            Text(
                                trayTask(run),
                                modifier = Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            run.startedAtMillis?.let { started ->
                                Text(
                                    compactDuration((now - started).coerceAtLeast(0)),
                                    color = signals.faint,
                                    style = MaterialTheme.typography.labelMedium,
                                    textAlign = TextAlign.End,
                                    modifier = Modifier.widthIn(min = 52.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun trayTask(run: SubagentRun): String = AgentsTray.task(run).ifBlank { run.identity }

/**
 * One agent as its own tab: what it was asked, the steps it took, its report,
 * and what the reader has sent it through the main chat.
 */
@Composable
internal fun AgentView(
    run: SubagentRun?,
    /** The reader's messages to this agent the lead chat holds. */
    sent: List<AgentRelay>,
    /** Those still waiting in the lead chat's queue. */
    queued: List<AgentRelay>,
    pendingAction: String?,
    onAction: (key: String, action: String) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (run == null) {
        Box(modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
            Text(
                "This agent isn't in the part of the chat that's loaded.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
        return
    }
    val signals = LocalXdSignals.current
    val running = run.state == SubagentState.RUNNING
    val signal = when (run.state) {
        SubagentState.RUNNING -> signals.working
        SubagentState.SUCCESS -> signals.faint
        SubagentState.FAILURE -> MaterialTheme.colorScheme.error
        SubagentState.FINISHED -> signals.faint
    }
    val now = rememberNow(running && run.startedAtMillis != null)
    val stats = run.statsAt(if (running) now else null)
    val scroll = rememberScrollState()
    // A message just sent lands at the foot, so follow it there.
    val messages = sent.size + queued.size
    var seen by remember { mutableIntStateOf(messages) }
    LaunchedEffect(messages) {
        if (messages > seen) scroll.animateScrollTo(scroll.maxValue)
        seen = messages
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .verticalScroll(scroll)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            BackendIcon(run.provider.lowercase(), size = 16.dp)
            val modelName = run.model.takeIf { it.isNotBlank() }?.let { DisplayLabels.model(it, run.provider) }
            Text(
                listOfNotNull(run.identity, modelName).joinToString(" · "),
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (run.background) StatusPill("Background", signals.faint)
            if (running) PulseDot(signal)
            Text(
                run.status,
                color = if (running) MaterialTheme.colorScheme.onSurfaceVariant else signal,
                style = MaterialTheme.typography.labelMedium,
            )
        }
        if (stats.isNotBlank()) {
            Text(stats, color = signals.faint, style = MaterialTheme.typography.labelSmall)
        }
        if (run.hasControls(pendingAction)) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) { SubagentControls(run, pendingAction, onAction) }
        }
        if (run.prompt.isNotBlank()) {
            SubagentSection("Task")
            UserBubble(run.prompt)
        }
        if (run.steps.isNotEmpty()) {
            val earlier = run.droppedSteps
            SubagentSection("Steps · ${run.steps.size + earlier}")
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(MaterialTheme.shapes.small)
                    .background(MaterialTheme.colorScheme.background)
                    .padding(8.dp),
            ) {
                if (earlier > 0) {
                    Text(
                        "$earlier earlier steps not kept",
                        color = signals.faint,
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
                run.steps.forEach { StepLine(it.tool, it.text) }
            }
        }
        val report = run.summary.ifBlank { run.latest }
        if (report.isNotBlank()) {
            SubagentSection(if (run.summary.isNotBlank()) "Report" else "Latest message")
            SelectionContainer { MarkdownText(report) }
        }
        if (run.prompt.isBlank() && run.steps.isEmpty() && report.isBlank()) {
            Text(
                "No task details reported.",
                color = signals.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        sent.forEach { UserBubble(it.text, caption = "Sent through the main chat") }
        queued.forEach { UserBubble(it.text, caption = "Queued in the main chat") }
        if (running) {
            Row(
                modifier = Modifier.padding(top = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                PulseDot(signals.working)
                Text(
                    run.activity.ifBlank { "Working" },
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** The reader's own words, in the bubble the chat draws them in, with an optional faint caption. */
@Composable
internal fun UserBubble(text: String, caption: String? = null) {
    Box(
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        contentAlignment = Alignment.CenterEnd,
    ) {
        Column(Modifier.fillMaxWidth(0.86f), horizontalAlignment = Alignment.End) {
            caption?.let {
                Text(
                    it,
                    modifier = Modifier.padding(start = 4.dp, end = 4.dp, bottom = 4.dp),
                    style = MaterialTheme.typography.labelMedium,
                    color = LocalXdSignals.current.faint,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Surface(
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                contentColor = MaterialTheme.colorScheme.onSurface,
                shape = MaterialTheme.shapes.large,
            ) {
                SelectionContainer(Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) { Text(text) }
            }
        }
    }
}

/**
 * The agent tab's composer. A message goes to the lead chat, which passes it
 * on; an agent that cannot take one says why in place of the field.
 */
@Composable
internal fun AgentComposer(
    run: SubagentRun?,
    draft: String,
    onDraft: (String) -> Unit,
    sending: Boolean,
    onSend: () -> Unit,
    compact: Boolean,
) {
    run ?: return
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .imePadding()
            .padding(horizontal = if (compact) 8.dp else 12.dp, vertical = if (compact) 6.dp else 10.dp),
    ) {
        if (!run.canTakeMessages) {
            Text(
                if (run.runsAsCommand) {
                    "This agent runs as a command, so it can't take messages."
                } else {
                    "Messages open once Claude reports this agent's id."
                },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 14.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
                textAlign = TextAlign.Center,
            )
            return@Column
        }
        val faint = LocalXdSignals.current.faint
        val canSend = draft.isNotBlank() && !sending
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = MaterialTheme.colorScheme.surfaceContainer,
            border = BorderStroke(1.dp, controlBorder()),
        ) {
            // As the chat's composer: send sits in a 48dp slot that is its touch target alone.
            Row(verticalAlignment = Alignment.Bottom) {
                BasicTextField(
                    value = draft,
                    onValueChange = onDraft,
                    modifier = Modifier.weight(1f).heightIn(min = ComposerSlot),
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.onSurface),
                    maxLines = 6,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { if (canSend) onSend() }),
                    decorationBox = { field ->
                        Box(
                            Modifier.padding(start = 16.dp, end = 4.dp, top = 12.dp, bottom = 12.dp),
                            contentAlignment = Alignment.CenterStart,
                        ) {
                            if (draft.isEmpty()) {
                                Text(
                                    "Message this agent…",
                                    style = MaterialTheme.typography.bodyLarge,
                                    color = faint,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            // Named by its placeholder, and once it holds text by its name in the field.
                            FieldName("Message this agent", draft.isNotEmpty(), field)
                        }
                    },
                )
                Box(Modifier.size(ComposerSlot), contentAlignment = Alignment.Center) {
                    FilledIconButton(
                        onClick = onSend,
                        enabled = canSend,
                        modifier = Modifier.size(40.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(
                            disabledContainerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                            disabledContentColor = faint,
                        ),
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_send),
                            contentDescription = if (sending) "Sending" else "Send",
                            modifier = Modifier.size(17.dp),
                        )
                    }
                }
            }
        }
    }
}
