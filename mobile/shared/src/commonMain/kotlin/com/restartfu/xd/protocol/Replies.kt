package com.restartfu.xd.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray

@Serializable
public data class FolderReply(
    val id: String,
    val name: String,
    val parent: String? = null,
    /** A folder of projects, which holds no repository and starts no sessions. */
    val group: Boolean = false,
    /** The GitHub account whose avatar stands in for the folder icon. */
    val owner: String? = null,
)

@Serializable
public data class ChatSummaryReply(
    val id: String,
    val folder: String,
    val title: String,
    val backend: String,
    val working: Boolean,
    val branch: String? = null,
    @SerialName("terminal_working")
    val terminalWorking: Boolean = false,
    /** Background agents and shells its agent process is still running. */
    @SerialName("background_tasks")
    val backgroundTasks: Int = 0,
    @SerialName("session_mode")
    val sessionMode: String = "chat",
    val session: String? = null,
)

@Serializable
public data class SessionSummaryReply(
    val id: String,
    val folder: String,
    val title: String,
    val chats: List<String> = emptyList(),
    val branch: String? = null,
    val working: Boolean = false,
    @SerialName("terminal_chat") val terminalChat: String = "session:$id",
    /** Seconds since the epoch when the session was settled; absent while it is active. */
    @SerialName("settled_at") val settledAt: Long? = null,
    @SerialName("auto_settle_disabled") val autoSettleDisabled: Boolean = false,
)

@Serializable
public data class TreeReply(
    val ok: Boolean,
    val folders: List<FolderReply>,
    val chats: List<ChatSummaryReply>,
    val sessions: List<SessionSummaryReply> = emptyList(),
    /** True when the host can settle sessions; hide settling otherwise. */
    val settlement: Boolean = false,
    /** True when the host keeps folders of projects; older hosts make a project instead. */
    @SerialName("folder_groups") val folderGroups: Boolean = false,
)

/** A chat checkout's Git state, as `git-status` and `git-commit` answer. */
@Serializable
public data class GitStatusReply(
    val ok: Boolean = true,
    val branch: String = "",
    val base: String = "",
    val upstream: String = "",
    val ahead: Int = 0,
    val behind: Int = 0,
    val staged: Int = 0,
    val unstaged: Int = 0,
    val untracked: Int = 0,
    val conflicted: Int = 0,
    val clean: Boolean = true,
) {
    /** Uncommitted files, of any kind. */
    val changed: Int get() = staged + unstaged + untracked + conflicted

    /** Everything can be committed in one go: something changed and nothing is in conflict. */
    val canCommit: Boolean get() = !clean && conflicted == 0

    /**
     * The branch is pushed, clean, and not the base, so it may have a pull
     * request worth asking GitHub about; the desktop asks under the same terms.
     */
    val mayHavePullRequest: Boolean
        get() = clean && conflicted == 0 && branch.isNotEmpty() &&
            branch != "(detached)" && branch != "(initial)" &&
            base.isNotEmpty() && branch != base && upstream.isNotEmpty() && ahead == 0
}

/**
 * The pull request of [branch], as `git-state` answers it. The host drops the
 * answer if HEAD moved during the lookup, so the link belongs to [branch].
 * An empty [url] means there is none; a null one means GitHub could not say.
 */
@Serializable
public data class PullRequestLookup(
    val branch: String,
    val url: String? = null,
)

/** The pull request of a chat's checked-out branch; an empty [url] means there is none. */
@Serializable
public data class PullRequestStatusReply(
    val ok: Boolean = true,
    val url: String = "",
)

@Serializable
public data class BranchSourceReply(
    val name: String,
    val reference: String,
    val oid: String,
)

@Serializable
public data class PullRequestSourceReply(
    val number: Long,
    val title: String,
    val oid: String,
)

/** What a project's new session can start from besides its checkouts. */
@Serializable
public data class NewChatSourcesReply(
    val ok: Boolean = true,
    val branches: List<BranchSourceReply> = emptyList(),
    @SerialName("pull_requests") val pullRequests: List<PullRequestSourceReply> = emptyList(),
    /** Why pull requests could not be listed, such as GitHub's CLI being missing. */
    @SerialName("pull_request_error") val pullRequestError: String? = null,
)

/** A project's checkouts, for choosing where a new session works. */
@Serializable
public data class FolderSettingsReply(
    val ok: Boolean = true,
    val worktrees: List<WorktreeReply> = emptyList(),
    @SerialName("can_create_worktree") val canCreateWorktree: Boolean = false,
    @SerialName("effective_workdir") val effectiveWorkdir: String? = null,
)

/** One assistant's sign-in on the host, as `agent-auth` and `agent-auth-changed` report it. */
@Serializable
public data class AgentAuthProviderReply(
    val provider: String,
    @SerialName("display_name") val displayName: String = provider,
    /** `unknown`, `signed-in`, `signed-out`, `signing-in`, or `failed`. */
    val state: String = "unknown",
    @SerialName("needs_input") val needsInput: Boolean = false,
    val detail: String? = null,
    @SerialName("login_url") val loginUrl: String? = null,
    @SerialName("device_code") val deviceCode: String? = null,
)

@Serializable
public data class AgentAuthReply(
    val ok: Boolean = true,
    val providers: List<AgentAuthProviderReply> = emptyList(),
)

/** One assistant CLI: its version, the latest release, and any update in progress. */
@Serializable
public data class AgentCliProviderReply(
    val provider: String,
    @SerialName("display_name") val displayName: String = provider,
    /** `idle`, `updating`, `updated`, or `failed`. */
    val state: String = "idle",
    val available: Boolean = false,
    val updatable: Boolean = false,
    val version: String? = null,
    val latest: String? = null,
    val detail: String? = null,
)

@Serializable
public data class AgentCliUpdatesReply(
    val ok: Boolean = true,
    val providers: List<AgentCliProviderReply> = emptyList(),
)

/** Whether new chats are titled, and the model titling each assistant's chats. */
@Serializable
public data class ChatTitleSettingsReply(
    val ok: Boolean = true,
    val enabled: Boolean = false,
    /** The model that will title each assistant's chats, or null if its CLI offers none. */
    val models: Map<String, String?> = emptyMap(),
)

@Serializable
public data class FolderContextReply(
    val ok: Boolean = true,
    val context: String? = null,
)

/** Secret names only; values never cross the wire. */
@Serializable
public data class AgentSecretsReply(
    val ok: Boolean = true,
    val names: List<String> = emptyList(),
)

@Serializable
public data class ResumeInterruptedReply(
    val ok: Boolean = true,
    val resumed: List<String> = emptyList(),
)

/** Token counts for one bucket of usage. */
@Serializable
public data class UsageTotalsReply(
    @SerialName("uncached_input") val uncachedInput: Long = 0,
    @SerialName("cached_input") val cachedInput: Long = 0,
    @SerialName("cache_creation") val cacheCreation: Long = 0,
    val output: Long = 0,
    val reasoning: Long = 0,
) {
    /** Everything sent and received, the way the desktop totals tokens. */
    val all: Long get() = uncachedInput + cachedInput + cacheCreation + output
}

/** One provider and model's usage in one day or hour of the window. */
@Serializable
public data class UsageBucketReply(
    val day: String,
    @SerialName("hour_start") val hourStart: String? = null,
    val provider: String,
    val model: String,
    val totals: UsageTotalsReply = UsageTotalsReply(),
    @SerialName("cost_usd") val costUsd: Double = 0.0,
    @SerialName("cost_source") val costSource: String = "",
    val records: Long = 0,
    @SerialName("unpriced_records") val unpricedRecords: Long = 0,
    @SerialName("unpriced_tokens") val unpricedTokens: Long = 0,
) {
    /** The period it falls in: its hour in an hourly window, else its day. */
    val period: String get() = hourStart ?: day
}

@Serializable
public data class UsagePeriodReply(
    val key: String,
    val label: String,
    val tooltip: String = label,
)

/** Cost and tokens over a window, read from the host's assistant transcripts. */
@Serializable
public data class UsageSummaryReply(
    @SerialName("read_at_ms") val readAtMs: Long = 0,
    /** `hour` or `day`. */
    val resolution: String = "day",
    @SerialName("window_label") val windowLabel: String = "",
    val periods: List<UsagePeriodReply> = emptyList(),
    val buckets: List<UsageBucketReply> = emptyList(),
    /** Sessions in the window, by provider. */
    val sessions: Map<String, Int> = emptyMap(),
    /** How each assistant's transcripts were read; a partial read means the totals miss some. */
    val sources: List<UsageSourceReply> = emptyList(),
)

@Serializable
public data class UsageSourceReply(
    val provider: String,
    /** `ok`, `missing` (not installed), or `partial`. */
    val status: String = "ok",
    val message: String? = null,
    @SerialName("skipped_files") val skippedFiles: Long = 0,
    @SerialName("malformed_records") val malformedRecords: Long = 0,
)

@Serializable
public data class UsageWindowReply(
    val id: String,
    val kind: String = "",
    val label: String,
    @SerialName("used_percent") val usedPercent: Double = 0.0,
    @SerialName("resets_at_ms") val resetsAtMs: Long? = null,
    @SerialName("resets_at_label") val resetsAtLabel: String? = null,
)

@Serializable
public data class ResetCreditsReply(
    @SerialName("available_count") val availableCount: Int = 0,
    @SerialName("next_expires_at_ms") val nextExpiresAtMs: Long? = null,
)

@Serializable
public data class UsageUnavailableReply(
    val reason: String = "",
    val message: String? = null,
)

/** One signed-in assistant account's subscription limits. */
@Serializable
public data class UsageAccountReply(
    val provider: String,
    @SerialName("account_key") val accountKey: String? = null,
    val plan: String? = null,
    @SerialName("checked_at_ms") val checkedAtMs: Long = 0,
    val windows: List<UsageWindowReply> = emptyList(),
    @SerialName("reset_credits") val resetCredits: ResetCreditsReply? = null,
    val unavailable: UsageUnavailableReply? = null,
)

/** A model's custom price, in USD per million tokens. */
@Serializable
public data class UsagePriceReply(
    val input: Double,
    val output: Double,
    @SerialName("cache_read") val cacheRead: Double? = null,
    @SerialName("cache_write") val cacheWrite: Double? = null,
)

@Serializable
public data class UsagePricesReply(
    val ok: Boolean = true,
    val prices: Map<String, UsagePriceReply> = emptyMap(),
    val aliases: Map<String, String> = emptyMap(),
    /** The host keeps its aliases when set-usage-prices leaves them out; older hosts clear them. */
    @SerialName("aliases_optional") val aliasesOptional: Boolean = false,
)

/** The host's rules for settling sessions by itself. */
@Serializable
public data class AutoSettleSettingsReply(
    val ok: Boolean = true,
    /** Days without activity before a session settles, or null when that rule is off. */
    @SerialName("after_days") val afterDays: Int? = null,
    @SerialName("on_merge") val onMerge: Boolean = false,
    @SerialName("keep_pull_requests") val keepPullRequests: Boolean = false,
)

@Serializable
public data class ShortcutsReply(
    val ok: Boolean,
    val global: List<String>,
    val workspace: List<String>,
    val effective: List<String>,
)

@Serializable
public data class WorktreeReply(
    val path: String,
    val branch: String? = null,
    val detached: Boolean,
    val main: Boolean,
    val current: Boolean,
)

@Serializable
public data class LiveItemReply(
    val tool: Boolean,
    val text: String,
    /** The stored row a live record (a subagent, a turn diff) rewrites in place. */
    @SerialName("message_id")
    val messageId: Long? = null,
)

@Serializable
public data class DraftAttachmentReply(
    val name: String = "image.png",
    val mime: String,
    val data: String,
)

@Serializable
public data class ChatReply(
    val interactions: List<InteractionReply> = emptyList(),
    @SerialName("interaction_revision") val interactionRevision: Long = 0,
    @SerialName("interaction_epoch") val interactionEpoch: String = "",    val ok: Boolean,
    val title: String,
    val backend: String,
    val commands: List<String> = emptyList(),
    val shortcuts: List<String> = emptyList(),
    val plan: Boolean,
    val fast: Boolean = false,
    val queued: String? = null,
    val queue: List<String> = emptyList(),
    @SerialName("queue_revision") val queueRevision: Long? = null,
    val draft: String = "",
    @SerialName("draft_revision")
    val draftRevision: Long = 0,
    @SerialName("draft_attachments")
    val draftAttachments: List<DraftAttachmentReply> = emptyList(),
    val working: Boolean,
    /** The last turn was cut short: stopped, interrupted, or ended by a usage limit. */
    val resumable: Boolean = false,
    val label: String? = null,
    // Present only while a turn is live. Together they mark exactly how much
    // of that turn this snapshot already contains, so the covered `text` and
    // `tool` events can be dropped instead of applied twice.
    @SerialName("turn_id")
    val turnId: Long? = null,
    @SerialName("turn_epoch") val turnEpoch: String = "",
    @SerialName("turn_sequence")
    val turnSequence: Long? = null,
    @SerialName("working_for")
    val workingFor: Long? = null,
    val items: List<LiveItemReply> = emptyList(),
    val segment: String? = null,
    val model: String? = null,
    val effort: String? = null,
    val access: String? = null,
    @SerialName("context_used")
    val contextUsed: Long? = null,
    @SerialName("context_window")
    val contextWindow: Long? = null,
    @SerialName("new_worktree")
    val newWorktree: Boolean,
    @SerialName("has_messages")
    val hasMessages: Boolean,
    val workdir: String? = null,
    @SerialName("linked_worktree")
    val linkedWorktree: Boolean? = null,
    val worktrees: List<WorktreeReply> = emptyList(),
    @SerialName("selected_worktree")
    val selectedWorktree: String? = null,
)

@Serializable
public data class MessageReply(
    val role: String,
    val content: String,
    val at: Long,
    val label: String? = null,
    val id: Long? = null,
)

@Serializable
public data class MessagesReply(
    val ok: Boolean,
    @SerialName("total_messages")
    val totalMessages: Int,
    @SerialName("last_message_id")
    val lastMessageId: Long,
    val messages: List<MessageReply>,
)

@Serializable
public data class DoneReply(
    val ok: Boolean,
    val id: String? = null,
    val session: String? = null,
    @SerialName("terminal_chat") val terminalChat: String? = null,
)

@Serializable
public data class WorkflowJobReply(
    val id: String,
    val name: String,
    val state: String,
    val conclusion: String? = null,
    val log: String? = null,
    @SerialName("started_at")
    val startedAt: Long? = null,
    @SerialName("completed_at")
    val completedAt: Long? = null,
)

@Serializable
public data class WorkflowStatusReply(
    val ok: Boolean,
    val name: String = "",
    val state: String = "",
    val conclusion: String? = null,
    val jobs: List<WorkflowJobReply> = emptyList(),
    @SerialName("started_at")
    val startedAt: Long? = null,
    @SerialName("completed_at")
    val completedAt: Long? = null,
)

@Serializable
public data class ModelReply(
    val id: String,
    val name: String,
    @SerialName("context_window")
    val contextWindow: Long = 0,
)

@Serializable
public data class BackendReply(
    val id: String,
    val name: String,
    @SerialName("default_model")
    val defaultModel: String = "",
    val models: List<ModelReply> = emptyList(),
    val efforts: List<String> = emptyList(),
)

@Serializable
public data class AgentCatalogReply(
    val ok: Boolean,
    val backends: List<BackendReply> = emptyList(),
)

@Serializable
public data class HostUpdateReply(
    val ok: Boolean,
    val version: String = "",
    val channel: String = "",
    val state: String = "idle",
    val supported: Boolean = false,
    val available: Boolean = false,
    val latest: String? = null,
    val error: String? = null,
)

@Serializable
public data class ImageReply(
    val ok: Boolean,
    val mime: String = "",
    val data: String = "",
)

@Serializable
public data class DiffReply(
    val ok: Boolean,
    val output: String = "",
)

@Serializable
public data class FileEntryReply(
    val name: String,
    val directory: Boolean,
)

@Serializable
public data class BrowseListReply(
    val ok: Boolean,
    val entries: List<FileEntryReply> = emptyList(),
)

@Serializable
public data class DirectoryListReply(
    val ok: Boolean,
    val path: String = "",
    val entries: List<String> = emptyList(),
)

@Serializable
public data class BrowseReadReply(
    val ok: Boolean,
    val content: String = "",
)

/**
 * [replay] is the host's bounded output history: `{"data": base64}` frames
 * with `{"columns","rows"}` resize frames between them. It is kept as raw JSON
 * because the order of the two kinds is the point -- a late device has to
 * interpret older output at the geometry that produced it.
 */
@Serializable
public data class TerminalReply(
    val id: String,
    val title: String = "",
    val agent: String? = null,
    val columns: Int,
    val rows: Int,
    val replay: JsonArray = JsonArray(emptyList()),
)

@Serializable
public data class TerminalListReply(
    val ok: Boolean,
    val terminals: List<TerminalReply> = emptyList(),
)

@Serializable
public data class TerminalOpenReply(
    val ok: Boolean,
    val id: String,
)

@Serializable
public data class InteractionReply(
    @SerialName("request_id") val requestId: String,
    val method: String,
    val params: kotlinx.serialization.json.JsonObject,
)

public fun InteractionReply.toModel(): com.restartfu.xd.model.PendingInteraction {
    fun text(value: kotlinx.serialization.json.JsonElement?): String? = (value as? kotlinx.serialization.json.JsonPrimitive)?.content
    val questions = (params["questions"] as? kotlinx.serialization.json.JsonArray).orEmpty().mapNotNull { element ->
        val q = element as? kotlinx.serialization.json.JsonObject ?: return@mapNotNull null
        com.restartfu.xd.model.PendingQuestion(
            id = text(q["id"]) ?: return@mapNotNull null,
            text = text(q["question"]) ?: "Your input is needed",
            options = (q["options"] as? kotlinx.serialization.json.JsonArray).orEmpty().mapNotNull { option -> text((option as? kotlinx.serialization.json.JsonObject)?.get("label")) },
            secret = text(q["isSecret"]) == "true",
        )
    }
    fun present(key: String): String? =
        (params[key] as? kotlinx.serialization.json.JsonPrimitive)
            ?.takeIf { it.isString }?.content?.takeIf { it.isNotBlank() }
    val command = present("command")
    return com.restartfu.xd.model.PendingInteraction(
        requestId = requestId, method = method, questions = questions,
        detail = command ?: present("reason") ?: params["permissions"]?.toString() ?: "The agent requests permission to continue",
        tool = present("tool"),
        reason = present("reason")?.takeIf { command != null && it != command },
        patch = present("patch"),
    )
}
