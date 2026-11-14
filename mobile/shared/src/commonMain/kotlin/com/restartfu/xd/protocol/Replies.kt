package com.restartfu.xd.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement

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
    /** Stopped by `pause-all` and not resumed or started again since. */
    val paused: Boolean = false,
    /** Linked pull requests, oldest first, on hosts with `linked_pull_requests`. */
    @SerialName("pull_requests")
    val pullRequests: List<LinkedPullRequestReply> = emptyList(),
    /** The chat whose agent started this one through xd's start_agent tool. */
    val parent: String? = null,
    /** Its last reply ends on a question for whoever reads it. */
    @SerialName("question_waiting")
    val questionWaiting: Boolean = false,
    /** Stopped at its usage quota, until the user gives it a new one. */
    @SerialName("quota_reached")
    val quotaReached: Boolean = false,
)

/** One pull request linked to a chat; [state] is absent until the host has asked GitHub. */
@Serializable
public data class LinkedPullRequestReply(
    val url: String,
    val state: String? = null,
)

/** What `link-pull-requests` kept, in canonical form, and what it refused. */
@Serializable
public data class LinkPullRequestsReply(
    val ok: Boolean = true,
    val linked: List<String> = emptyList(),
    val invalid: List<String> = emptyList(),
)

/** Whether `unlink-pull-request` removed a link that was there. */
@Serializable
public data class UnlinkPullRequestReply(
    val ok: Boolean = true,
    val changed: Boolean = false,
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
    /** The session whose agent started this one's through xd's start_agent tool. */
    val parent: String? = null,
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
    /** True when the host can pause every chat at once; hide the pause button otherwise. */
    @SerialName("pause_all") val pauseAll: Boolean = false,
    /** True when the host can pause one folder's chats; older hosts would pause every chat. */
    @SerialName("pause_folders") val pauseFolders: Boolean = false,
    /** True when the host keeps pull requests linked to chats and lists them per chat. */
    @SerialName("linked_pull_requests") val linkedPullRequests: Boolean = false,
    /** True when the host can limit chats to a share of their account's usage. */
    @SerialName("chat_quotas") val chatQuotas: Boolean = false,
    /** True when a chat can be set to run on one of its assistant's accounts. */
    @SerialName("chat_accounts") val chatAccounts: Boolean = false,
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

/**
 * One assistant's sign-ins on the host, as `agent-auth` and
 * `agent-auth-changed` report them. The top-level sign-in fields describe the
 * user's own CLI sign-in, the `default` account, as hosts from before
 * accounts sent them; [accounts] lists every account in priority order.
 */
@Serializable
public data class AgentAuthProviderReply(
    val provider: String,
    @SerialName("display_name") val displayName: String = provider,
    /** `unknown`, `checking`, `signed-in`, `signed-out`, `signing-in`, `signing-out`, or `failed`. */
    val state: String = "unknown",
    @SerialName("needs_input") val needsInput: Boolean = false,
    val detail: String? = null,
    @SerialName("login_url") val loginUrl: String? = null,
    @SerialName("device_code") val deviceCode: String? = null,
    /** Empty from a host that keeps only the user's own sign-in. */
    val accounts: List<AgentAccountReply> = emptyList(),
    /** Whether a turn turned away for usage continues on the next account. */
    @SerialName("auto_switch") val autoSwitch: Boolean = false,
    /** A new account is signing in. */
    val adding: Boolean = false,
    /** The outcome of the last attempt to add an account. */
    val notice: String? = null,
) {
    /** Whether the host can keep more than the user's own sign-in. */
    public val holdsSeveral: Boolean get() = accounts.isNotEmpty()

    /**
     * Every account, in priority order. A host from before accounts reports
     * only the user's own sign-in, at the top level, so that is one account.
     */
    public fun accountList(): List<AgentAccountReply> = accounts.ifEmpty {
        listOf(
            AgentAccountReply(
                id = AgentAccountReply.DEFAULT,
                default = true,
                label = "$displayName sign-in",
                state = state,
                detail = detail,
                loginUrl = loginUrl,
                deviceCode = deviceCode,
                needsInput = needsInput,
                inUse = true,
            ),
        )
    }

    /** The account the provider's one sign-in is running for, if any. */
    public fun signingIn(): AgentAccountReply? = accountList().firstOrNull { it.state == "signing-in" }

    /** A sign-in or sign-out is running, which holds every other account action back. */
    public val busy: Boolean
        get() = accountList().any { it.busy || it.state == "signing-in" || it.state == "signing-out" }

    /** Whether another account can be added now. */
    public val canAdd: Boolean get() = holdsSeveral && !busy && !adding

    /** The accounts that have a place in the order, without one still signing in for the first time. */
    public fun rankedAccounts(): List<AgentAccountReply> = accountList().filterNot { it.pending }

    /**
     * The order with the ranked account at [index] moved one place up or
     * down, or null when it cannot move that way.
     */
    public fun orderMoving(index: Int, up: Boolean): List<String>? {
        val order = rankedAccounts().map { it.id }.toMutableList()
        val other = if (up) index - 1 else index + 1
        if (index !in order.indices || other !in order.indices) return null
        order[index] = order[other].also { order[other] = order[index] }
        return order
    }
}

/** One of an assistant's accounts on the host. */
@Serializable
public data class AgentAccountReply(
    val id: String,
    /** The user's own CLI sign-in, in the CLI's usual home. */
    val default: Boolean = false,
    /** Still signing in for the first time; it joins the list once that succeeds. */
    val pending: Boolean = false,
    /** The address when the host knows it. */
    val label: String = "",
    val email: String? = null,
    val plan: String? = null,
    val state: String = "unknown",
    val detail: String? = null,
    @SerialName("login_url") val loginUrl: String? = null,
    @SerialName("device_code") val deviceCode: String? = null,
    @SerialName("needs_input") val needsInput: Boolean = false,
    /** The account a new chat would start on. */
    @SerialName("in_use") val inUse: Boolean = false,
    /** The account the provider's one sign-in or sign-out is running for. */
    val busy: Boolean = false,
    /** `mismatch` or `duplicate`: either keeps the account from taking turns. */
    val problem: String? = null,
    @SerialName("limited_until_ms") val limitedUntilMs: Long? = null,
    /** Such as `5-hour limit`. */
    @SerialName("limited_reason") val limitedReason: String? = null,
    @SerialName("checked_at_ms") val checkedAtMs: Long? = null,
    val windows: List<AgentAccountWindowReply> = emptyList(),
) {
    public companion object {
        public const val DEFAULT: String = "default"
    }
}

/** One of an account's usage allowances, as the accounts list carries it. */
@Serializable
public data class AgentAccountWindowReply(
    /** `session`, `weekly`, or another window. */
    val kind: String = "",
    val label: String = "",
    @SerialName("used_percent") val usedPercent: Double = 0.0,
    @SerialName("resets_at_ms") val resetsAtMs: Long? = null,
)

@Serializable
public data class AgentAuthReply(
    val ok: Boolean = true,
    val providers: List<AgentAuthProviderReply> = emptyList(),
    /** False on a host that keeps only the user's own sign-in. */
    @SerialName("multiple_accounts") val multipleAccounts: Boolean = false,
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

/** Whether `set-chat-quota` continued a chat that had stopped at its quota. */
@Serializable
public data class SetChatQuotaReply(
    val ok: Boolean = true,
    val resumed: Boolean = false,
)

/** The chats `pause-all` stopped. */
@Serializable
public data class PauseAllReply(
    val ok: Boolean = true,
    val paused: List<String> = emptyList(),
)

/** The paused chats `resume-all` continued. */
@Serializable
public data class ResumeAllReply(
    val ok: Boolean = true,
    val resumed: List<String> = emptyList(),
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
    /** The account's 1-based place in the host's list, sent only when the assistant has several. */
    @SerialName("account_rank") val accountRank: Int? = null,
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
    /** The account the chat is set to run on; absent while the host chooses. */
    val account: String? = null,
    @SerialName("context_used")
    val contextUsed: Long? = null,
    @SerialName("context_window")
    val contextWindow: Long? = null,
    /**
     * `{tokens, after_ms}` when a plain send into this idle Claude chat can
     * compact first. Kept as raw JSON and read with
     * [com.restartfu.xd.model.CompactBeforeSend.parse], so an unexpected
     * shape means no offer rather than an unreadable chat.
     */
    @SerialName("compact_before_send")
    val compactBeforeSend: JsonElement? = null,
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
    /**
     * `{percent, used, own}` while a quota applies to the chat. Kept as raw
     * JSON and read with [com.restartfu.xd.model.ChatQuota.parse], so an
     * unexpected shape means no quota rather than an unreadable chat.
     */
    val quota: JsonElement? = null,
    /** Stopped at its quota, until the user gives it a new one. */
    @SerialName("quota_reached")
    val quotaReached: Boolean = false,
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
    /** Whether rows older than this page exist; only cursor-aware hosts say. */
    @SerialName("has_older")
    val hasOlder: Boolean? = null,
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

/** A bounded, read-only snapshot of a task still owned by a kept agent process. */
@Serializable
public data class BackgroundTaskOutput(
    val id: String,
    val kind: String = "Task",
    val description: String = "Background task",
    @SerialName("started_at") val startedAt: Long = 0,
    val lines: List<BackgroundOutputLine> = emptyList(),
    @SerialName("output_error") val outputError: String? = null,
    val truncated: Boolean = false,
)

@Serializable
public data class BackgroundOutputLine(val text: String, val error: Boolean = false)

@Serializable
public data class BackgroundTaskOutputReply(
    val ok: Boolean,
    val chat: String,
    val tasks: List<BackgroundTaskOutput> = emptyList(),
)
