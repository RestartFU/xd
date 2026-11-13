package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.height
import androidx.compose.ui.platform.testTag
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.Ask
import com.restartfu.xd.model.AskBlock
import com.restartfu.xd.model.ChatState
import com.restartfu.xd.model.PendingInteraction
import com.restartfu.xd.model.PendingQuestion

/**
 * What waits on the reader, docked at the top of the composer rather than
 * carded above it. One shows at a time: the agent's own requests first,
 * since its turn is held on them, then a quota stop, then a tagged question.
 */
internal sealed interface DockedAsk {
    /** An approval or the agent's questions; [waiting] counts every request held. */
    data class Interaction(val request: PendingInteraction, val waiting: Int) : DockedAsk

    data object Quota : DockedAsk

    data class Tagged(val ask: Ask) : DockedAsk
}

internal fun dockedAsk(state: ChatState): DockedAsk? {
    state.interactions.firstOrNull()?.let { return DockedAsk.Interaction(it, state.interactions.size) }
    if (state.quotaReached && !state.working) return DockedAsk.Quota
    return AskBlock.pending(state)?.let(DockedAsk::Tagged)
}

/** The one of the agent's questions showing, and whether all of them are answered and on their way. */
internal data class ShowingQuestion(val index: Int, val question: PendingQuestion, val sent: Boolean)

/**
 * The first question not answered yet. With every one answered the answers
 * are on their way, so the last stays up, without taking another answer,
 * until the host says the request is done.
 */
internal fun PendingInteraction.showing(answers: Map<String, String>): ShowingQuestion? {
    if (questions.isEmpty()) return null
    val index = questions.indexOfFirst { it.id !in answers }
    return if (index >= 0) {
        ShowingQuestion(index, questions[index], sent = false)
    } else {
        ShowingQuestion(questions.lastIndex, questions.last(), sent = true)
    }
}

/** The composer's placeholder while a question takes a typed answer. */
internal const val TYPED_ANSWER_PLACEHOLDER = "Or type your own answer…"

/** The composer's placeholder while a chat is stopped at its quota. */
internal const val QUOTA_PLACEHOLDER = "Messages you send wait until it goes on…"

/** Content inside the ask section keeps this much more inset than its option rows. */
internal val AskInset = 6.dp

/**
 * The ask section's frame and the hairline under it. Never taller than about
 * half of what the keyboard leaves, so the message field stays in reach; a
 * taller ask scrolls inside itself. A [footer], such as an approval's
 * answers, stays put under the part that scrolls, so it is in reach however
 * long that part is.
 */
@Composable
internal fun AskFrame(
    footer: (@Composable () -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val keyboard = WindowInsets.ime.asPaddingValues().calculateBottomPadding()
    val room = LocalConfiguration.current.screenHeightDp.dp - keyboard
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(max = (room * 0.5f).coerceAtLeast(160.dp))
            .padding(start = 10.dp, end = 10.dp, top = 16.dp, bottom = 10.dp),
    ) {
        // Takes what the footer leaves, and no more than it needs.
        Column(
            modifier = Modifier
                .weight(1f, fill = false)
                .testTag(ASK_SCROLL_TAG)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            content = content,
        )
        if (footer != null) {
            Spacer(Modifier.height(12.dp))
            footer()
        }
    }
    HorizontalDivider(
        modifier = Modifier.padding(horizontal = 16.dp),
        color = MaterialTheme.colorScheme.outlineVariant,
    )
}

/**
 * The ring and title an ask starts with. The ring is the only colour: the
 * header, tab and sidebar already say the chat waits, so nothing repeats it.
 */
@Composable
internal fun AskHeader(title: String, counter: String? = null) {
    Row(
        modifier = Modifier.padding(horizontal = AskInset),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // Centred on the title's first line, so a question that wraps keeps it at its start.
        Box(
            Modifier
                .padding(top = 7.dp)
                .size(10.dp)
                .border(1.5.dp, LocalXdSignals.current.waiting, CircleShape),
        )
        Text(
            title,
            // Told to a screen reader as it arrives: an ask docks under the
            // transcript, where nothing else would say the agent now waits.
            modifier = Modifier.weight(1f).semantics {
                heading()
                liveRegion = LiveRegionMode.Polite
            },
            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
        )
        counter?.let {
            Text(
                it,
                modifier = Modifier.padding(top = 4.dp),
                style = MaterialTheme.typography.labelMedium.copy(fontFeatureSettings = "tnum"),
                color = LocalXdSignals.current.faint,
            )
        }
    }
}

/**
 * One answer, as a plain row: a faint number, the answer, and an arrow while
 * it is pressed or has keyboard focus. Tapping it answers.
 */
@Composable
internal fun AskOptionRow(number: Int, label: String, enabled: Boolean, onPick: () -> Unit) {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val focused by source.collectIsFocusedAsState()
    val lit = enabled && (pressed || focused)
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 50.dp)
            .clip(MaterialTheme.shapes.medium)
            .background(if (lit) MaterialTheme.colorScheme.surfaceContainerHighest else Color.Transparent)
            .clickable(
                interactionSource = source,
                indication = LocalIndication.current,
                enabled = enabled,
                role = Role.Button,
                onClick = onPick,
            )
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text(
            "$number",
            // The answer is what a screen reader reads; the number is only a mark.
            modifier = Modifier.widthIn(min = 10.dp).clearAndSetSemantics {},
            style = MaterialTheme.typography.bodyMedium.copy(fontFeatureSettings = "tnum"),
            color = faint,
        )
        Text(
            label,
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.bodyLarge,
            color = if (enabled) MaterialTheme.colorScheme.onSurface else faint,
        )
        Icon(
            painterResource(R.drawable.ic_send),
            contentDescription = null,
            tint = faint,
            modifier = Modifier.size(16.dp).rotate(90f).alpha(if (lit) 1f else 0f),
        )
    }
}

@Composable
private fun AskOptions(options: List<String>, enabled: Boolean, onPick: (String) -> Unit) {
    if (options.isEmpty()) return
    Column(Modifier.fillMaxWidth()) {
        options.forEachIndexed { index, option ->
            AskOptionRow(index + 1, option, enabled) { onPick(option) }
        }
    }
}

/**
 * A tagged question. Both bundled assistants run non-interactively, so a
 * question is a block in the reply rather than a prompt; an option is sent
 * as the next message, which is exactly what typing it would do. One that
 * takes typed text takes it from the composer.
 */
@Composable
internal fun TaggedAsk(ask: Ask, enabled: Boolean, onAnswer: (String) -> Unit) {
    AskFrame {
        AskHeader(ask.question)
        AskOptions(ask.options, enabled, onAnswer)
    }
}

/**
 * One of the agent's questions, one at a time, counted when it asked
 * several. Typed answers come from the composer; a secret one is typed here,
 * hidden, since the composer's draft is kept by the host and shown on
 * every device.
 */
@Composable
internal fun AgentQuestionAsk(
    request: PendingInteraction,
    showing: ShowingQuestion,
    waiting: Int,
    onAnswer: (questionId: String, answer: String) -> Unit,
) {
    val question = showing.question
    val counter = when {
        request.questions.size > 1 -> "${showing.index + 1} / ${request.questions.size}"
        waiting > 1 -> "1 / $waiting"
        else -> null
    }
    AskFrame {
        AskHeader(question.text, counter)
        AskOptions(question.options, enabled = !showing.sent) { onAnswer(question.id, it) }
        if (question.secret) SecretAnswer(request.requestId, question, enabled = !showing.sent, onAnswer)
    }
}

@Composable
private fun SecretAnswer(
    requestId: String,
    question: PendingQuestion,
    enabled: Boolean,
    onAnswer: (questionId: String, answer: String) -> Unit,
) {
    // Not saved with the screen's state: a secret stays out of the saved bundle.
    var typed by remember(requestId, question.id) { mutableStateOf("") }
    val submit = { if (typed.isNotBlank()) onAnswer(question.id, typed) }
    OutlinedTextField(
        value = typed,
        onValueChange = { typed = it },
        modifier = Modifier.fillMaxWidth().padding(horizontal = AskInset),
        enabled = enabled,
        singleLine = true,
        placeholder = { Text("Type your answer…", color = LocalXdSignals.current.faint) },
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Send),
        keyboardActions = KeyboardActions(onSend = { submit() }),
        trailingIcon = {
            IconButton(onClick = submit, enabled = enabled && typed.isNotBlank()) {
                Icon(painterResource(R.drawable.ic_send), "Send answer", modifier = Modifier.size(20.dp))
            }
        },
    )
}

internal fun approvalTitle(request: PendingInteraction): String =
    when (request.tool) {
        "Bash" -> "Run this command?"
        "Edit", "MultiEdit", "NotebookEdit" -> "Apply this edit?"
        "Write" -> "Write this file?"
        "WebFetch" -> "Fetch this page?"
        "WebSearch" -> "Search the web?"
        "ExitPlanMode" -> "Approve the plan?"
        null -> when (request.method) {
            "item/fileChange/requestApproval" -> "Apply these changes?"
            "item/commandExecution/requestApproval" -> "Run this command?"
            else -> "Approval needed"
        }
        else -> "Allow ${request.tool}?"
    }

/** Buttons sized to sit three to a phone's row. */
internal val AskButtonPadding = PaddingValues(horizontal = 16.dp)

/**
 * An approval: what the agent wants to run or change, why, and the answer.
 * Counted when more than one waits; they are answered in order.
 */
@Composable
@OptIn(ExperimentalLayoutApi::class)
internal fun ApprovalAsk(
    request: PendingInteraction,
    waiting: Int,
    onDecide: (decision: String) -> Unit,
) {
    AskFrame(
        footer = {
            FlowRow(
                modifier = Modifier.fillMaxWidth().padding(horizontal = AskInset),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Button(onClick = { onDecide("accept") }, contentPadding = AskButtonPadding) { Text("Allow once") }
                FilledTonalButton(
                    onClick = { onDecide("acceptForSession") },
                    contentPadding = AskButtonPadding,
                    colors = askSecondaryButtonColors(),
                ) { Text("Allow for session") }
                Spacer(Modifier.weight(1f))
                TextButton(onClick = { onDecide("decline") }) {
                    Text("Deny", color = MaterialTheme.colorScheme.error)
                }
            }
        },
    ) {
        AskHeader(approvalTitle(request), if (waiting > 1) "1 / $waiting" else null)
        Text(
            request.detail,
            modifier = Modifier
                .padding(horizontal = AskInset)
                .fillMaxWidth()
                .heightIn(max = 140.dp)
                .clip(MaterialTheme.shapes.small)
                .background(MaterialTheme.colorScheme.surfaceContainerLow)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp, vertical = 10.dp),
            fontFamily = XdMonoFamily,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurface,
        )
        request.reason?.let {
            Text(
                it,
                modifier = Modifier.padding(horizontal = AskInset),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        request.patch?.let { patch -> Box(Modifier.padding(horizontal = AskInset)) { DiffText(patch) } }
    }
}

/** The part of an ask that scrolls, for tests. */
internal const val ASK_SCROLL_TAG = "ask-scroll"

/** A quiet filled button: the composer's raised neutral, in either theme. */
@Composable
internal fun askSecondaryButtonColors() = ButtonDefaults.filledTonalButtonColors(
    containerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
    contentColor = MaterialTheme.colorScheme.onSurface,
)
