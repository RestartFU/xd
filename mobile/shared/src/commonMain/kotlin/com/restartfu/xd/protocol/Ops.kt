package com.restartfu.xd.protocol

import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.add
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

public enum class ChatOption(
    public val wire: String,
) {
    MODEL("model"),
    EFFORT("effort"),
    ACCESS("access"),
    PLAN("plan"),
    FAST("fast"),
    BACKEND("backend"),
    NEW_WORKTREE("new-worktree"),
    WORKSPACE("workspace"),

    /** The account the chat runs on; an empty value lets the host choose. */
    ACCOUNT("account"),
}

public object Ops {
    public fun tree(): JsonObject = op("tree")

    public fun backgroundTaskOutput(chatId: String): JsonObject = withChat("background-task-output", chatId)

    public fun chat(chatId: String): JsonObject = withChat("chat", chatId)

    public fun shortcuts(folderId: String? = null): JsonObject = buildJsonObject {
        put("op", "shortcuts")
        folderId?.let { put("folder", it) }
    }

    public fun setShortcuts(
        folderId: String? = null,
        prompts: List<String>,
    ): JsonObject = buildJsonObject {
        require(prompts.size <= 24) { "A shortcut list can contain at most 24 prompts" }
        prompts.forEach { prompt ->
            require(prompt.encodeToByteArray().size <= 4096) {
                "A shortcut prompt can contain at most 4096 bytes"
            }
        }
        put("op", "set-shortcuts")
        folderId?.let { put("folder", it) }
        put(
            "shortcuts",
            buildJsonArray { prompts.forEach(::add) },
        )
    }

    @OptIn(ExperimentalEncodingApi::class)
    public fun setDraft(
        chatId: String,
        text: String,
        images: List<PngAttachment>? = null,
    ): JsonObject = buildJsonObject {
        require(text.encodeToByteArray().size <= 1024 * 1024) {
            "Message draft is too large"
        }
        put("op", "set-draft")
        put("chat", chatId)
        put("text", text)
        if (images != null) {
            Limits.validateImages(images)
            put(
                "attachments",
                buildJsonArray {
                    images.forEach { image ->
                        add(
                            buildJsonObject {
                                put("name", "image.png")
                                put("mime", Limits.PNG_MIME)
                                put("data", Base64.Default.encode(image.bytes))
                            },
                        )
                    }
                },
            )
        }
    }

    /**
     * The newest [limit] messages, or with [before] the [limit] stored just
     * before that message id. A host without cursors ignores [before] and
     * answers without `has_older`.
     */
    public fun messages(chatId: String, limit: Int = 150, before: Long? = null): JsonObject = buildJsonObject {
        put("op", "messages")
        put("chat", chatId)
        put("limit", limit)
        if (before != null) put("before", before)
    }

    public fun workflowStatus(marker: String): JsonObject = buildJsonObject {
        require(marker.isNotBlank()) { "A workflow marker is required" }
        put("op", "workflow-status")
        put("text", marker)
    }

    @OptIn(ExperimentalEncodingApi::class)
    public fun send(
        chatId: String,
        text: String,
        images: List<PngAttachment> = emptyList(),
        /** Steer a message that would be queued into the running turn instead. */
        steer: Boolean = false,
        /**
         * Run `/compact` first in an idle chat that offered it with
         * `compact_before_send`; the message waits right behind it.
         */
        compactFirst: Boolean = false,
    ): JsonObject {
        require(text.isNotEmpty() || images.isNotEmpty()) {
            "A message needs text or an image"
        }
        Limits.validateImages(images)

        return buildJsonObject {
            put("op", "send")
            put("chat", chatId)
            put("text", text)
            if (steer) put("steer", true)
            if (compactFirst) put("compact_first", true)
            if (images.isNotEmpty()) {
                put(
                    "attachments",
                    buildJsonArray {
                        images.forEach { image ->
                            val encodedLength = ((image.bytes.size + 2) / 3) * 4
                            Limits.validateEncodedImageLength(encodedLength)
                            add(
                                buildJsonObject {
                                    put("mime", Limits.PNG_MIME)
                                    put("data", Base64.Default.encode(image.bytes))
                                },
                            )
                        }
                    },
                )
            }
        }
    }

    /** Stops a running agent ("stop") or sends one the turn waits on to the background ("background"). */
    public fun subagent(chatId: String, key: String, action: String): JsonObject = buildJsonObject {
        require(action == "stop" || action == "background") { "Unknown agent action" }
        put("op", "subagent")
        put("chat", chatId)
        put("key", key)
        put("action", action)
    }

    public fun queue(chatId: String, text: String): JsonObject = buildJsonObject {
        require(text.isNotEmpty()) { "Queued text must not be empty" }
        put("op", "queue")
        put("chat", chatId)
        put("text", text)
    }

    /**
     * Drops queued message [index], or the whole queue without one. [text] is
     * what the client saw there; the host refuses if another message now holds
     * that place. Hosts that predate the check ignore it.
     */
    public fun dropQueue(chatId: String, index: Int?, text: String? = null, revision: Long? = null): JsonObject = buildJsonObject {
        put("op", "drop-queue")
        put("chat", chatId)
        if (index != null) {
            require(index >= 0) { "Queue index must not be negative" }
            put("index", index)
            if (text != null) put("text", text)
        }
        // The queue revision as seen, clearing the whole queue included: equal
        // texts, and messages added since, are only told apart by it.
        if (revision != null) put("queue_revision", revision)
    }

    /**
     * Replaces a queued message.
     *
     * [oldText] is what the client believes is there. The host uses it to
     * refuse an edit aimed at a queue another device has already changed.
     */
    public fun editQueue(
        chatId: String,
        index: Int,
        oldText: String,
        text: String,
        revision: Long? = null,
    ): JsonObject = buildJsonObject {
        require(index >= 0) { "Queue index must not be negative" }
        require(text.isNotEmpty()) { "Edited text must not be empty" }
        if (revision != null) put("queue_revision", revision)
        put("op", "edit-queue")
        put("chat", chatId)
        put("index", index)
        put("old-text", oldText)
        put("text", text)
    }

    /**
     * Redirects the running turn to a queued message, so the agent takes it up
     * immediately instead of finishing first. The host sends it into the turn
     * when the agent can take that, and otherwise promotes it to the front of
     * the queue and stops the turn.
     *
     * [text] must be the message currently at [index]; the host refuses the
     * steer otherwise rather than redirecting the agent to the wrong thing.
     */
    public fun steerQueue(
        chatId: String,
        index: Int,
        text: String,
        revision: Long? = null,
    ): JsonObject = buildJsonObject {
        require(index >= 0) { "Queue index must not be negative" }
        if (revision != null) put("queue_revision", revision)
        put("op", "steer-queue")
        put("chat", chatId)
        put("index", index)
        put("text", text)
    }

    public fun cancel(chatId: String): JsonObject = withChat("cancel", chatId)

    /**
     * Deletes a chat and everything it owns.
     *
     * The host forgets the agent session, kills the chat's terminals and
     * removes the stored transcript. There is no undo, so a client should ask
     * first.
     */
    public fun deleteChat(chatId: String): JsonObject = withChat("delete-chat", chatId)

    /** Renames a chat. The host refuses a blank title rather than clearing it. */
    public fun renameChat(chatId: String, title: String): JsonObject = buildJsonObject {
        require(title.isNotBlank()) { "A chat needs a title" }
        put("op", "rename-chat")
        put("chat", chatId)
        put("title", title)
    }

    /** Renames a workspace session; its chats keep their own titles. */
    public fun renameSession(sessionId: String, title: String): JsonObject = buildJsonObject {
        require(title.isNotBlank()) { "A session needs a title" }
        put("op", "rename-session")
        put("session", requireId(sessionId, "Session"))
        put("title", title)
    }

    /**
     * Deletes a session with every chat in it, stops its agents and closes its
     * terminals. Checkout files and worktrees stay. There is no undo.
     */
    public fun deleteSession(sessionId: String): JsonObject =
        withSession("delete-session", sessionId)

    /**
     * Moves finished work to the Settled shelf. The host refuses while a member
     * chat is running, has a queued message, or waits on an approval.
     */
    public fun settleSession(sessionId: String): JsonObject =
        withSession("settle-session", sessionId)

    /** Brings a settled session back and keeps the automatic rules off it until new activity. */
    public fun unsettleSession(sessionId: String): JsonObject =
        withSession("unsettle-session", sessionId)

    /** Keeps a session out of the automatic rules for good, or hands it back to them. */
    public fun setSessionAutoSettle(sessionId: String, enabled: Boolean): JsonObject = buildJsonObject {
        put("op", "set-session-auto-settle")
        put("session", requireId(sessionId, "Session"))
        put("enabled", enabled)
    }

    public fun autoSettleSettings(): JsonObject = op("auto-settle-settings")

    /**
     * Changes the host's auto-settle rules. Only the members given are sent;
     * [afterDays] of [AfterDays.Off] turns the inactivity rule off.
     */
    public fun setAutoSettleSettings(
        afterDays: AfterDays? = null,
        onMerge: Boolean? = null,
        keepPullRequests: Boolean? = null,
    ): JsonObject = buildJsonObject {
        put("op", "set-auto-settle-settings")
        when (afterDays) {
            null -> Unit
            AfterDays.Off -> put("after_days", JsonNull)
            is AfterDays.Days -> {
                require(afterDays.days in 1..90) { "Auto-settle takes 1 to 90 days" }
                put("after_days", afterDays.days)
            }
        }
        if (onMerge != null) put("on_merge", onMerge)
        if (keepPullRequests != null) put("keep_pull_requests", keepPullRequests)
    }

    /** The inactivity rule's setting: off, or a number of days. */
    public sealed interface AfterDays {
        public data object Off : AfterDays
        public data class Days(val days: Int) : AfterDays
    }

    public fun renameFolder(folderId: String, name: String): JsonObject = buildJsonObject {
        require(
            name.isNotBlank() && !name.startsWith('.') && '/' !in name && '\\' !in name
        ) { "A project name cannot be empty or hidden, or contain a path separator" }
        put("op", "rename-folder")
        put("folder", requireId(folderId, "Folder"))
        put("name", name)
    }

    /**
     * Removes a project and its sessions from xd; the host moves its files to
     * the workspace trash.
     */
    public fun trashFolder(folderId: String): JsonObject = buildJsonObject {
        put("op", "trash-folder")
        put("folder", requireId(folderId, "Folder"))
    }

    /** Continues a cut-short turn; the host refuses if the chat moved on. */
    public fun resumeTurn(chatId: String): JsonObject = withChat("resume-turn", chatId)

    /** The host's default chat quota, on hosts whose `tree` has `chat_quotas`. */
    public fun chatQuotaSettings(): JsonObject = op("chat-quota-settings")

    /** Sets the host's default chat quota, a whole percentage from 1 to 100; null limits no chat. */
    public fun setChatQuotaSettings(percent: Int?): JsonObject = buildJsonObject {
        put("op", "set-chat-quota-settings")
        put("percent", percent?.let { JsonPrimitive(requirePercent(it)) } ?: JsonNull)
    }

    /** When the host compacts an idle Claude chat: `onReturn`, `beforeCacheExpires` or `off`. */
    public fun idleCompactionSettings(): JsonObject = op("idle-compaction-settings")

    /** Sets when the host compacts an idle Claude chat. */
    public fun setIdleCompactionSettings(mode: com.restartfu.xd.model.IdleCompactionMode): JsonObject = buildJsonObject {
        put("op", "set-idle-compaction-settings")
        put("mode", mode.wire)
    }

    /**
     * Gives one chat a quota of its own, no limit, or the host's default
     * again. Its count starts again, and a chat that stopped at its quota
     * goes on.
     */
    public fun setChatQuota(chatId: String, quota: ChatQuotaChoice): JsonObject = buildJsonObject {
        put("op", "set-chat-quota")
        put("chat", requireId(chatId, "Chat"))
        when (quota) {
            is ChatQuotaChoice.Percent -> put("percent", requirePercent(quota.percent))
            ChatQuotaChoice.Unlimited -> put("percent", JsonNull)
            ChatQuotaChoice.Inherit -> put("inherit", true)
        }
    }

    /** What [setChatQuota] gives a chat. */
    public sealed interface ChatQuotaChoice {
        /** Its own quota, a whole percentage from 1 to 100. */
        public data class Percent(val percent: Int) : ChatQuotaChoice
        /** No limit at all. */
        public data object Unlimited : ChatQuotaChoice
        /** The host's default, following it as it changes. */
        public data object Inherit : ChatQuotaChoice
    }

    private fun requirePercent(percent: Int): Int {
        require(percent in 1..100) { "A chat quota is a whole percentage from 1 to 100" }
        return percent
    }

    /**
     * Stops every working chat in folder [folderId] and the folders inside it,
     * and every one waiting on background tasks, marking them paused.
     */
    public fun pauseAll(folderId: String): JsonObject = buildJsonObject {
        put("op", "pause-all")
        put("folder", requireId(folderId, "Folder"))
    }

    /** Continues every chat in folder [folderId] that [pauseAll] stopped. */
    public fun resumeAll(folderId: String): JsonObject = buildJsonObject {
        put("op", "resume-all")
        put("folder", requireId(folderId, "Folder"))
    }

    /**
     * Moves queued message [source] before or [after] [anchor]. Both texts are
     * what the client saw there, so a queue another device changed is refused
     * rather than reordered wrongly.
     */
    public fun reorderQueue(
        chatId: String,
        source: Int,
        sourceText: String,
        anchor: Int,
        anchorText: String,
        after: Boolean,
        revision: Long? = null,
    ): JsonObject = buildJsonObject {
        require(source >= 0 && anchor >= 0) { "Queue indexes must not be negative" }
        if (revision != null) put("queue_revision", revision)
        put("op", "reorder-queue")
        put("chat", chatId)
        put("source", source)
        put("source-text", sourceText)
        put("anchor", anchor)
        put("anchor-text", anchorText)
        put("after", after)
    }

    public fun newChat(
        folderId: String,
        title: String? = null,
        backend: String? = null,
        sessionId: String? = null,
        worktree: NewSessionWorktree? = null,
        startFromRemote: Boolean = true,
        effort: String? = null,
        access: String? = null,
        plan: Boolean? = null,
        /** The account the chat runs on, or null to let the host choose. */
        account: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "new-chat")
        account?.let { put("account", it) }
        effort?.let { put("effort", it) }
        access?.let { put("access", it) }
        plan?.let { put("plan", it) }
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        put("folder", folderId)
        if (sessionId != null) {
            require(sessionId.isNotBlank()) { "Session id must not be blank" }
            // A chat joining a session shares its checkout; the host refuses a worktree.
            require(worktree == null) { "A chat joining a session cannot choose a worktree" }
            put("session", sessionId)
        }
        if (!title.isNullOrBlank()) put("title", title)
        if (!backend.isNullOrBlank()) put("backend", backend)
        if (worktree != null) put("worktree", worktree.wire())
        if (worktree is NewSessionWorktree.Branch) put("start_from_remote", startFromRemote)
    }

    /**
     * Where a new session works: a fresh worktree, one of the project's
     * checkouts, or a managed worktree for a branch or pull request. A branch
     * or pull request names the commit the host listed, which it checks again
     * before checking out.
     */
    public sealed interface NewSessionWorktree {
        public data object New : NewSessionWorktree
        public data class Existing(val path: String) : NewSessionWorktree
        public data class Branch(val reference: String, val oid: String) : NewSessionWorktree
        public data class PullRequest(val number: Long, val oid: String) : NewSessionWorktree

        public fun wire(): JsonObject = buildJsonObject {
            when (val worktree = this@NewSessionWorktree) {
                New -> put("kind", "new")
                is Existing -> {
                    put("kind", "existing")
                    put("path", worktree.path)
                }
                is Branch -> {
                    put("kind", "branch")
                    put("ref", worktree.reference)
                    put("oid", worktree.oid)
                }
                is PullRequest -> {
                    put("kind", "pull-request")
                    put("number", worktree.number)
                    put("oid", worktree.oid)
                }
            }
        }
    }

    public fun newChatSources(folderId: String): JsonObject = buildJsonObject {
        put("op", "new-chat-sources")
        put("folder", requireId(folderId, "Folder"))
    }

    public fun folderSettings(folderId: String): JsonObject = buildJsonObject {
        put("op", "folder-settings")
        put("folder", requireId(folderId, "Folder"))
    }

    /**
     * The assistants' sign-ins and accounts. With [check] false the host
     * answers without checking each account again, which is enough to offer
     * them for a chat.
     */
    public fun agentAuth(check: Boolean = true): JsonObject = buildJsonObject {
        put("op", "agent-auth")
        if (!check) put("check", false)
    }

    /**
     * Starts, feeds, or cancels an assistant's sign-in on the host, adds an
     * account, or signs one out. [account] names the account to sign in again
     * or sign out, the user's own sign-in when left out; any other account is
     * removed when signed out.
     */
    public fun agentAuthAction(
        action: AuthAction,
        provider: String,
        input: String? = null,
        account: String? = null,
    ): JsonObject = buildJsonObject {
        require(provider in AUTH_PROVIDERS) { "Unknown assistant: $provider" }
        put("op", action.wire)
        put("provider", provider)
        if (action == AuthAction.INPUT) {
            require(!input.isNullOrEmpty()) { "Sign-in input must not be empty" }
            put("input", input)
        }
        if (account != null) {
            require(action == AuthAction.START || action == AuthAction.LOGOUT) { "Only signing in or out names an account" }
            require(account.isNotBlank()) { "Account must not be blank" }
            put("account", account)
        }
    }

    public enum class AuthAction(public val wire: String) {
        START("agent-auth-start"),
        ADD("agent-auth-add"),
        INPUT("agent-auth-input"),
        CANCEL("agent-auth-cancel"),
        LOGOUT("agent-auth-logout"),
    }

    /** Sets the order turns use an assistant's accounts in; [accounts] names each account once. */
    public fun agentAccountsOrder(provider: String, accounts: List<String>): JsonObject = buildJsonObject {
        require(provider in AUTH_PROVIDERS) { "Unknown assistant: $provider" }
        require(accounts.isNotEmpty() && accounts.none(String::isBlank)) { "The order must name each account" }
        require(accounts.toSet().size == accounts.size) { "The order must name each account once" }
        put("op", "agent-accounts-order")
        put("provider", provider)
        put("accounts", buildJsonArray { accounts.forEach { add(JsonPrimitive(it)) } })
    }

    /** Turns moving a chat to the next account when one runs out on or off. */
    public fun agentAccountsAutoSwitch(provider: String, enabled: Boolean): JsonObject = buildJsonObject {
        require(provider in AUTH_PROVIDERS) { "Unknown assistant: $provider" }
        put("op", "agent-accounts-auto-switch")
        put("provider", provider)
        put("enabled", enabled)
    }

    /** `status`, `check`, or `install` (which needs a [provider]). */
    public fun agentCliUpdates(action: String = "status", provider: String? = null): JsonObject = buildJsonObject {
        require(action in setOf("status", "check", "install")) { "Unknown CLI update action: $action" }
        require(action != "install" || !provider.isNullOrBlank()) { "Installing an update needs a provider" }
        put("op", "agent-cli-updates")
        put("action", action)
        if (!provider.isNullOrBlank()) put("provider", provider)
    }

    public fun chatTitleSettings(): JsonObject = op("chat-title-settings")

    /** Turns titling on or off. */
    public fun setChatTitlesEnabled(enabled: Boolean): JsonObject = buildJsonObject {
        put("op", "set-chat-title-settings")
        put("enabled", enabled)
    }

    /** Chooses the model that titles [backend]'s chats; null goes back to the built-in choice. */
    public fun setChatTitleModel(backend: String, model: String?): JsonObject = buildJsonObject {
        require(backend in setOf("codex", "claude")) { "Only Codex and Claude chats are titled" }
        put("op", "set-chat-title-settings")
        put("backend", backend)
        put("model", model?.let(::JsonPrimitive) ?: JsonNull)
    }

    public fun folderContext(folderId: String): JsonObject = buildJsonObject {
        put("op", "folder-context")
        put("folder", requireId(folderId, "Folder"))
    }

    /** Replaces a project's context; blank clears it. The host keeps at most 256 KiB. */
    public fun setFolderContext(folderId: String, context: String?): JsonObject = buildJsonObject {
        put("op", "set-folder-context")
        put("folder", requireId(folderId, "Folder"))
        val trimmed = context?.trim()?.takeIf(String::isNotEmpty)
        put("context", trimmed?.let(::JsonPrimitive) ?: JsonNull)
    }

    /** Secret names for a project, or the host-wide ones without [folderId]. */
    public fun agentSecrets(folderId: String? = null): JsonObject = buildJsonObject {
        put("op", "agent-secrets")
        if (folderId != null) put("folder", requireId(folderId, "Folder"))
    }

    /**
     * Replaces a secret set. An entry without a value keeps the stored one, so
     * values never have to come back to the phone to be kept.
     */
    public fun setAgentSecrets(folderId: String?, entries: List<SecretEntry>): JsonObject = buildJsonObject {
        require(entries.map(SecretEntry::name).toSet().size == entries.size) { "Secret names must be unique" }
        put("op", "set-agent-secrets")
        if (folderId != null) put("folder", requireId(folderId, "Folder"))
        put("entries", buildJsonArray {
            entries.forEach { entry ->
                require(SECRET_NAME.matches(entry.name)) { "Not an environment variable name: ${entry.name}" }
                add(buildJsonObject {
                    put("name", entry.name)
                    entry.value?.let {
                        require(it.isNotEmpty()) { "A replacement secret needs a value" }
                        put("value", it)
                    }
                })
            }
        })
    }

    /** A secret to keep ([value] null) or to set. */
    public data class SecretEntry(val name: String, val value: String? = null)

    private val SECRET_NAME = Regex("[A-Za-z_][A-Za-z0-9_]*")
    private val AUTH_PROVIDERS = setOf("codex", "claude")

    /** Continues, or drops, every chat a host restart cut off since the last answer. */
    public fun resumeInterrupted(resume: Boolean): JsonObject = buildJsonObject {
        put("op", "resume-interrupted")
        put("resume", resume)
    }

    /**
     * Asks for a usage summary over [days]; the host answers `pending` and
     * sends a `usage-summary` event carrying [generation].
     */
    public fun usageSummary(days: Int, timeZone: String, generation: Long, refresh: Boolean = false): JsonObject = buildJsonObject {
        require(days in USAGE_DAYS) { "Usage covers 1, 7, 30, or 90 days" }
        put("op", "usage-summary")
        put("days", days)
        put("time_zone", timeZone)
        put("generation", generation)
        if (refresh) put("refresh", true)
    }

    public val USAGE_DAYS: List<Int> = listOf(1, 7, 30, 90)

    /** Asks each assistant for its limits; answered by a `usage-limits` event carrying [generation]. */
    public fun usageLimits(timeZone: String, generation: Long): JsonObject = buildJsonObject {
        put("op", "usage-limits")
        put("time_zone", timeZone)
        put("generation", generation)
    }

    /**
     * Spends one banked Codex reset credit on [accountKey]. [idempotencyKey]
     * names the attempt and is reused when retrying it. This cannot be undone.
     */
    public fun usageConsumeResetCredit(accountKey: String, idempotencyKey: String): JsonObject = buildJsonObject {
        require(accountKey.isNotBlank() && idempotencyKey.isNotBlank()) { "A reset needs the account and an attempt key" }
        put("op", "usage-consume-reset-credit")
        put("provider", "codex")
        put("account_key", accountKey)
        put("idempotency_key", idempotencyKey)
    }

    public fun usagePrices(): JsonObject = op("usage-prices")

    /**
     * Replaces the custom prices, and the aliases too when [aliases] is given;
     * without it the host keeps its saved ones. A blank cache rate uses the
     * input rate.
     */
    public fun setUsagePrices(prices: Map<String, UsagePriceReply>, aliases: Map<String, String>? = null): JsonObject = buildJsonObject {
        put("op", "set-usage-prices")
        put("prices", buildJsonObject {
            prices.forEach { (model, price) ->
                require(model.isNotBlank()) { "A price needs a model" }
                require(price.input >= 0 && price.output >= 0) { "Prices cannot be negative" }
                put(model, buildJsonObject {
                    put("input", price.input)
                    put("output", price.output)
                    price.cacheRead?.let { put("cache_read", it) }
                    price.cacheWrite?.let { put("cache_write", it) }
                })
            }
        })
        if (aliases != null) put("aliases", buildJsonObject { aliases.forEach { (model, target) -> put(model, target) } })
    }

    public fun gitStatus(chatId: String): JsonObject = withChat("git-status", chatId)

    /** Commits every change in the chat's checkout with [message], as the desktop's Commit does. */
    public fun gitCommit(chatId: String, message: String): JsonObject = buildJsonObject {
        val trimmed = message.trim()
        require(trimmed.isNotEmpty() && trimmed.encodeToByteArray().size <= 4096 && '\u0000' !in trimmed) {
            "Commit messages must contain 1 to 4096 bytes"
        }
        put("op", "git-commit")
        put("chat", chatId)
        put("message", trimmed)
    }

    public fun gitPullRequestStatus(chatId: String): JsonObject = withChat("git-pr-status", chatId)

    /**
     * Links pull requests to a chat. The host keeps the GitHub-style ones in
     * canonical form and answers the rest as `invalid`; it takes 1 to
     * [MAX_PULL_REQUEST_LINKS] at once.
     */
    public fun linkPullRequests(chatId: String, urls: List<String>): JsonObject = buildJsonObject {
        val cleaned = urls.map(String::trim).filter(String::isNotEmpty)
        require(cleaned.size in 1..MAX_PULL_REQUEST_LINKS) {
            "Link 1 to $MAX_PULL_REQUEST_LINKS pull requests at a time"
        }
        put("op", "link-pull-requests")
        put("chat", requireId(chatId, "Chat"))
        put("urls", buildJsonArray { cleaned.forEach { add(JsonPrimitive(it)) } })
    }

    /** Removes one linked pull request from a chat; the host's automatic links will not bring it back. */
    public fun unlinkPullRequest(chatId: String, url: String): JsonObject = buildJsonObject {
        require(url.isNotBlank()) { "A pull request link is required" }
        put("op", "unlink-pull-request")
        put("chat", requireId(chatId, "Chat"))
        put("url", url)
    }

    /** The most addresses one `link-pull-requests` request takes. */
    public const val MAX_PULL_REQUEST_LINKS: Int = 16

    /**
     * Asks for the checkout's Git state and its branch's pull request. The
     * host acknowledges, then publishes a `git-state` event carrying [request].
     */
    public fun gitState(chatId: String, request: String): JsonObject = buildJsonObject {
        require(request.isNotBlank()) { "A Git state request needs a correlation string" }
        put("op", "git-state")
        put("chat", chatId)
        put("request", request)
        put("pull_request", true)
    }

    public fun newFolder(
        name: String,
        parentId: String? = null,
        repository: String? = null,
        repositoryUrl: String? = null,
        /** A folder of projects rather than a project. */
        group: Boolean = false,
        /** The GitHub account the folder shows the avatar of; implies [group]. */
        owner: String? = null,
    ): JsonObject = buildJsonObject {
        require(
            name.isNotBlank() &&
                !name.startsWith('.') &&
                '/' !in name &&
                '\\' !in name
        ) { "A folder name cannot be empty or hidden, or contain a path separator" }
        require(parentId == null || parentId.isNotBlank()) { "Parent id must not be blank" }
        val folder = group || owner != null
        require(!folder || (repository.isNullOrBlank() && repositoryUrl.isNullOrBlank())) {
            "A folder holds projects, not a repository"
        }
        put("op", "new-folder")
        put("name", name)
        if (parentId != null) put("parent", parentId)
        if (folder) put("group", true)
        if (owner != null) put("owner", owner)
        if (!repository.isNullOrBlank()) put("repo", repository)
        // The host clones this into the folder it makes, and checks the
        // address itself: only it knows what its Git can reach.
        if (!repositoryUrl.isNullOrBlank()) put("repo_url", repositoryUrl)
    }

    /** The avatar of a GitHub account a folder names as its `owner`. */
    public fun ownerIcon(owner: String): JsonObject = buildJsonObject {
        require(owner.isNotBlank()) { "An owner is required" }
        put("op", "owner-icon")
        put("owner", owner)
    }

    public fun listDirectories(path: String? = null): JsonObject = buildJsonObject {
        put("op", "list-dir")
        if (!path.isNullOrBlank()) put("path", path)
    }

    public fun moveFolder(
        folderId: String,
        parentId: String? = null,
    ): JsonObject = buildJsonObject {
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        require(parentId == null || parentId.isNotBlank()) { "Parent id must not be blank" }
        put("op", "move-folder")
        put("folder", folderId)
        if (parentId != null) put("parent", parentId)
    }

    public fun moveChat(
        chatId: String,
        folderId: String,
    ): JsonObject = buildJsonObject {
        require(chatId.isNotBlank()) { "Chat id must not be blank" }
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        put("op", "move-chat")
        put("chat", chatId)
        put("folder", folderId)
    }

    /**
     * Reads a stored image.
     *
     * The host only serves paths inside its own remote-paste directory, so
     * this cannot be pointed at arbitrary files. [preview] asks for a scaled
     * copy, which is all a transcript needs.
     */
    public fun imageRead(path: String, preview: Boolean = true): JsonObject = buildJsonObject {
        require(path.isNotBlank()) { "An image path is required" }
        put("op", "image-read")
        put("path", path)
        put("preview", preview)
    }

    /** The assistants and models this host can run. */
    public fun agentCatalog(): JsonObject = op("agent-catalog")

    /**
     * Asks about, or performs, an update of the host itself.
     *
     * `install` replaces the files, which is safe while turns run; `restart`
     * drops every attached device and loses the running turn, so it is a
     * separate action nobody takes by accident.
     */
    public fun hostUpdate(action: String = "status"): JsonObject = buildJsonObject {
        require(action in HOST_UPDATE_ACTIONS) { "No such host-update action" }
        put("op", "host-update")
        put("action", action)
    }

    /** Selects a model without changing the assistant fixed to this chat. */
    public fun selectModel(
        chatId: String,
        model: String,
    ): JsonObject = buildJsonObject {
        require(model.isNotBlank()) { "A model id is required" }
        put("op", "set-option")
        put("chat", chatId)
        put("option", ChatOption.MODEL.wire)
        put("value", model)
    }

    public fun ping(): JsonObject = op("ping")

    public fun setOption(
        chatId: String,
        option: ChatOption,
        value: String,
    ): JsonObject = buildJsonObject {
        put("op", "set-option")
        put("chat", chatId)
        put("option", option.wire)
        put("value", value)
    }

    public fun setBoolOption(
        chatId: String,
        option: ChatOption,
        value: Boolean,
    ): JsonObject = setOption(chatId, option, if (value) "true" else "false")

    /**
     * Reads a whole patch in one call.
     *
     * The desktop also asks per file to build its collapsible sections; a
     * phone shows one scrollable patch, so `working-all` and `branch-all` are
     * all it needs. `branch-all` wants the base from a prior `base` read.
     */
    public fun diffRead(
        chatId: String,
        read: String,
        base: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "diff-read")
        put("chat", chatId)
        put("read", read)
        if (!base.isNullOrBlank()) put("base", base)
    }

    public fun listDirectory(chatId: String, path: String?): JsonObject = buildJsonObject {
        put("op", "file-browse")
        put("chat", chatId)
        put("action", "list")
        if (!path.isNullOrEmpty()) put("path", path)
    }

    public fun readFile(chatId: String, path: String): JsonObject = buildJsonObject {
        require(path.isNotBlank()) { "A file path is required" }
        put("op", "file-browse")
        put("chat", chatId)
        put("action", "read")
        put("path", path)
    }

    /**
     * Saves a file.
     *
     * [original] is what was read, and the host refuses the write if the file
     * no longer matches it: the agent edits the same tree, and a save that
     * simply overwrote would silently drop whatever it had just done.
     */
    public fun writeFile(
        chatId: String,
        path: String,
        original: String,
        content: String,
    ): JsonObject = buildJsonObject {
        require(path.isNotBlank()) { "A file path is required" }
        put("op", "file-browse")
        put("chat", chatId)
        put("action", "write")
        put("path", path)
        put("original", original)
        put("content", content)
    }

    public fun terminalList(chatId: String): JsonObject = withChat("terminal-list", chatId)

    public fun terminalOpen(
        chatId: String,
        columns: Int,
        rows: Int,
        reuse: Boolean,
    ): JsonObject = buildJsonObject {
        require(columns > 0 && rows > 0) { "A terminal needs a positive size" }
        put("op", "terminal-open")
        put("chat", chatId)
        put("columns", columns)
        put("rows", rows)
        put("reuse", reuse)
    }

    /** [data] is base64: the pty takes bytes, not text. */
    public fun terminalInput(terminalId: String, data: String): JsonObject = buildJsonObject {
        put("op", "terminal-input")
        put("terminal", terminalId)
        put("data", data)
    }

    public fun terminalResize(
        terminalId: String,
        columns: Int,
        rows: Int,
    ): JsonObject = buildJsonObject {
        require(columns > 0 && rows > 0) { "A terminal needs a positive size" }
        put("op", "terminal-resize")
        put("terminal", terminalId)
        put("columns", columns)
        put("rows", rows)
    }

    public fun terminalKill(terminalId: String): JsonObject = buildJsonObject {
        put("op", "terminal-kill")
        put("terminal", terminalId)
    }

    private val HOST_UPDATE_ACTIONS = setOf("status", "check", "install", "restart")

    private fun op(name: String): JsonObject = buildJsonObject {
        put("op", name)
    }

    private fun requireId(id: String, kind: String): String {
        require(id.isNotBlank()) { "$kind id must not be blank" }
        return id
    }

    private fun withSession(name: String, sessionId: String): JsonObject = buildJsonObject {
        put("op", name)
        put("session", requireId(sessionId, "Session"))
    }

    private fun withChat(name: String, chatId: String): JsonObject = buildJsonObject {
        put("op", name)
        put("chat", chatId)
    }
}
