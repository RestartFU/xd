#include <glib/gstdio.h>
#include <sqlite3.h>

#include "storage/storage.h"

typedef struct
{
  char *dir;
  XdStorage *storage;
} Fixture;

static void
fixture_set_up (Fixture       *fixture,
                gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *db_path = NULL;

  fixture->dir = g_dir_make_tmp ("xd-storage-XXXXXX", &error);
  g_assert_no_error (error);

  db_path = g_build_filename (fixture->dir, "chats.db", NULL);
  fixture->storage = xd_storage_new (db_path, &error);
  g_assert_no_error (error);
  g_assert_nonnull (fixture->storage);
}

static void
fixture_tear_down (Fixture       *fixture,
                   gconstpointer  user_data)
{
  g_clear_object (&fixture->storage);

  /* WAL leaves siblings behind, so clear the directory rather than the file. */
  if (fixture->dir != NULL)
    {
      g_autoptr (GDir) dir = g_dir_open (fixture->dir, 0, NULL);
      const char *name;

      while (dir != NULL && (name = g_dir_read_name (dir)) != NULL)
        {
          g_autofree char *path = g_build_filename (fixture->dir, name, NULL);
          g_remove (path);
        }

      g_rmdir (fixture->dir);
      g_clear_pointer (&fixture->dir, g_free);
    }
}

static void
test_create_and_list (Fixture       *fixture,
                      gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) chats = NULL;
  g_autofree char *first = NULL;
  g_autofree char *second = NULL;
  const XdChat *chat;

  first = xd_storage_create_chat (fixture->storage, "folder-a", "Rate limiting",
                                  "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);
  g_assert_nonnull (first);

  second = xd_storage_create_chat (fixture->storage, "folder-b", "Elsewhere",
                                   "codex", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  chats = xd_storage_list_chats (fixture->storage, "folder-a", &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chats->len, ==, 1);

  chat = g_ptr_array_index (chats, 0);
  g_assert_cmpstr (chat->id, ==, first);
  g_assert_cmpstr (chat->title, ==, "Rate limiting");
  g_assert_cmpstr (chat->backend, ==, "claude");
}

static void
test_chats_follow_latest_user_message (Fixture       *fixture,
                                       gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) chats = NULL;
  g_autofree char *first = NULL;
  g_autofree char *second = NULL;

  first = xd_storage_create_chat (fixture->storage, "folder", "First",
                                  "claude", NULL, NULL, NULL, &error);
  g_usleep (1000);
  second = xd_storage_create_chat (fixture->storage, "folder", "Second",
                                   "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  chats = xd_storage_list_chats (fixture->storage, "folder", &error);
  g_assert_cmpstr (((XdChat *) g_ptr_array_index (chats, 0))->id, ==, second);

  g_usleep (1000);
  g_assert_true (xd_storage_append_message (
    fixture->storage, first, "user", "work here", NULL, NULL, &error));
  g_clear_pointer (&chats, g_ptr_array_unref);
  chats = xd_storage_list_chats (fixture->storage, "folder", &error);
  g_assert_cmpstr (((XdChat *) g_ptr_array_index (chats, 0))->id, ==, first);

  /* Agent output and metadata writes must not steal user-selected recency. */
  g_usleep (1000);
  g_assert_true (xd_storage_append_message (
    fixture->storage, second, "assistant", "finished", NULL, NULL, &error));
  g_assert_true (xd_storage_set_chat_title (
    fixture->storage, second, "Renamed", &error));
  g_clear_pointer (&chats, g_ptr_array_unref);
  chats = xd_storage_list_chats (fixture->storage, "folder", &error);
  g_assert_cmpstr (((XdChat *) g_ptr_array_index (chats, 0))->id, ==, first);

  g_usleep (1000);
  g_assert_true (xd_storage_append_message (
    fixture->storage, second, "user", "work here now", NULL, NULL, &error));
  g_clear_pointer (&chats, g_ptr_array_unref);
  chats = xd_storage_list_chats (fixture->storage, "folder", &error);
  g_assert_cmpstr (((XdChat *) g_ptr_array_index (chats, 0))->id, ==, second);
  g_assert_no_error (error);
}

static void
test_new_chats_inherit_last_changed_agent (Fixture       *fixture,
                                           gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *changed_id = NULL;
  g_autofree char *before_id = NULL;
  g_autofree char *after_id = NULL;
  g_autoptr (XdChat) before = NULL;
  g_autoptr (XdChat) after = NULL;

  changed_id = xd_storage_create_chat (
    fixture->storage, "folder-a", "Changed",
    "claude", "claude-opus-5", "medium", NULL, &error);
  g_assert_no_error (error);

  /* Merely creating a chat is not a changed preference. Folder defaults still
   * apply until the user touches an agent option. */
  before_id = xd_storage_create_chat (
    fixture->storage, "folder-b", "Before",
    "codex", "gpt-5.4", "low", NULL, &error);
  g_assert_no_error (error);
  before = xd_storage_get_chat (fixture->storage, before_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (before->backend, ==, "codex");
  g_assert_cmpstr (before->model, ==, "gpt-5.4");
  g_assert_cmpstr (before->effort, ==, "low");

  g_assert_true (xd_storage_set_backend (
    fixture->storage, changed_id, "codex", &error));
  g_assert_true (xd_storage_set_model (
    fixture->storage, changed_id, "gpt-5.6-codex", &error));
  g_assert_true (xd_storage_set_effort (
    fixture->storage, changed_id, "xhigh", &error));
  g_assert_true (xd_storage_set_access (
    fixture->storage, changed_id, "full", &error));
  g_assert_true (xd_storage_set_plan (
    fixture->storage, changed_id, TRUE, &error));
  g_assert_no_error (error);

  /* Folder fallbacks now lose to the complete last-changed configuration. */
  after_id = xd_storage_create_chat (
    fixture->storage, "folder-b", "After",
    "claude", "claude-haiku-4-5", "low", NULL, &error);
  g_assert_no_error (error);
  after = xd_storage_get_chat (fixture->storage, after_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (after->backend, ==, "codex");
  g_assert_cmpstr (after->model, ==, "gpt-5.6-codex");
  g_assert_cmpstr (after->effort, ==, "xhigh");
  g_assert_cmpstr (after->access, ==, "full");
  g_assert_true (after->plan);
}

/* Chats hang off a folder's UUID, never its path: that is what lets a folder
 * be renamed or moved on disk without losing its conversations. */
static void
test_chats_follow_folder_id (Fixture       *fixture,
                             gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) chats = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "stable-uuid", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  chats = xd_storage_list_chats (fixture->storage, "stable-uuid", &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chats->len, ==, 1);
  g_assert_cmpstr (((XdChat *) g_ptr_array_index (chats, 0))->id, ==, chat_id);
}

static void
test_messages_round_trip (Fixture       *fixture,
                          gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (xd_storage_append_message (fixture->storage, chat_id, "user",
                                            "how do I add a rate limiter?",
                                            NULL, NULL, &error));
  g_assert_no_error (error);

  g_assert_true (xd_storage_append_message (fixture->storage, chat_id, "assistant",
                                            "Use a token bucket.",
                                            "{\"type\":\"result\"}",
                                            "Claude Opus 5 · High", &error));
  g_assert_no_error (error);

  messages = xd_storage_list_messages (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 2);

  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 0))->role, ==, "user");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 1))->content, ==,
                   "Use a token bucket.");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 1))->raw_json, ==,
                   "{\"type\":\"result\"}");

  /* What produced a reply belongs to the reply. Changing the chat's model
   * afterwards must not rewrite what the earlier ones were answered by. */
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 1))->label, ==,
                   "Claude Opus 5 · High");
  g_assert_null (((XdMessage *) g_ptr_array_index (messages, 0))->label);

  g_assert_true (xd_storage_set_model (fixture->storage, chat_id, "claude-haiku-4-5", &error));
  g_assert_no_error (error);

  g_ptr_array_unref (messages);
  messages = xd_storage_list_messages (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 1))->label, ==,
                   "Claude Opus 5 · High");
}

static void
test_live_message_can_be_removed (Fixture       *fixture,
                                  gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  gint64 message_id = 0;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_true (xd_storage_append_message_with_id (
    fixture->storage, chat_id, "assistant", "<workspace>",
    NULL, NULL, &message_id, &error));
  g_assert_cmpint (message_id, >, 0);
  g_assert_true (xd_storage_delete_message (
    fixture->storage, message_id, &error));
  g_assert_no_error (error);

  messages = xd_storage_list_messages (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 0);
}

static void
test_recent_messages_are_bounded (Fixture       *fixture,
                                  gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;
  guint total = 0;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  for (guint i = 0; i < 5; i++)
    {
      g_autofree char *content = g_strdup_printf ("message-%u", i);

      g_assert_true (xd_storage_append_message (
        fixture->storage, chat_id, i % 2 == 0 ? "user" : "assistant",
        content, "{\"large\":\"backend event\"}", NULL, &error));
    }
  g_assert_no_error (error);

  messages = xd_storage_list_recent_messages (
    fixture->storage, chat_id, 2, &total, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (total, ==, 5);
  g_assert_cmpuint (messages->len, ==, 2);
  g_assert_cmpstr (
    ((XdMessage *) g_ptr_array_index (messages, 0))->content, ==, "message-3");
  g_assert_cmpstr (
    ((XdMessage *) g_ptr_array_index (messages, 1))->content, ==, "message-4");
  g_assert_null (((XdMessage *) g_ptr_array_index (messages, 0))->raw_json);
  g_assert_null (((XdMessage *) g_ptr_array_index (messages, 1))->raw_json);
}

/*
 * A session id only means something to the CLI that issued it, so a chat that
 * has talked to both keeps one of each. Switching assistants and back must
 * resume each side rather than starting over.
 */
static void
test_sessions_are_per_backend (Fixture       *fixture,
                               gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *claude_session = NULL;
  g_autofree char *codex_session = NULL;
  g_autofree char *missing = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (xd_storage_set_session_id (fixture->storage, chat_id,
                                            "claude", "sess-claude", &error));
  g_assert_true (xd_storage_set_session_id (fixture->storage, chat_id,
                                            "codex", "sess-codex", &error));
  g_assert_no_error (error);

  claude_session = xd_storage_get_session_id (fixture->storage, chat_id,
                                              "claude", &error);
  codex_session = xd_storage_get_session_id (fixture->storage, chat_id,
                                             "codex", &error);
  g_assert_no_error (error);
  g_assert_cmpstr (claude_session, ==, "sess-claude");
  g_assert_cmpstr (codex_session, ==, "sess-codex");

  /* A backend the chat has never used has nothing to resume. */
  missing = xd_storage_get_session_id (fixture->storage, chat_id, "nobody", &error);
  g_assert_no_error (error);
  g_assert_null (missing);
}

/* Sessions expire CLI-side; forgetting one must not disturb the other. */
static void
test_forgetting_one_session (Fixture       *fixture,
                             gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *claude_session = NULL;
  g_autofree char *codex_session = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  xd_storage_set_session_id (fixture->storage, chat_id, "claude", "sess-a", &error);
  xd_storage_set_session_id (fixture->storage, chat_id, "codex", "sess-b", &error);
  g_assert_no_error (error);

  g_assert_true (xd_storage_set_session_id (fixture->storage, chat_id,
                                            "claude", NULL, &error));
  g_assert_no_error (error);

  claude_session = xd_storage_get_session_id (fixture->storage, chat_id,
                                              "claude", &error);
  codex_session = xd_storage_get_session_id (fixture->storage, chat_id,
                                             "codex", &error);
  g_assert_null (claude_session);
  g_assert_cmpstr (codex_session, ==, "sess-b");
}

/*
 * Resuming a session restores only what that assistant was sent. Anything
 * said to the other one in between has to be replayed, so each backend tracks
 * how far through the conversation it has been brought.
 */
static void
test_each_backend_tracks_what_it_has_seen (Fixture       *fixture,
                                           gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) unseen = NULL;
  g_autofree char *chat_id = NULL;
  gint64 after_claude;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);

  /* Claude answers the first exchange. */
  xd_storage_append_message (fixture->storage, chat_id, "user", "who are you", NULL, NULL, &error);
  xd_storage_append_message (fixture->storage, chat_id, "assistant", "Claude here", NULL, NULL, &error);
  g_assert_no_error (error);

  after_claude = xd_storage_last_message_id (fixture->storage, chat_id);
  g_assert_true (xd_storage_set_last_seen (fixture->storage, chat_id, "claude",
                                           after_claude, &error));

  /* Then the user switches to Codex for a turn. */
  xd_storage_append_message (fixture->storage, chat_id, "user", "and you?", NULL, NULL, &error);
  xd_storage_append_message (fixture->storage, chat_id, "assistant", "Codex here", NULL, NULL, &error);
  g_assert_no_error (error);

  /* Claude has never been told about that exchange. */
  g_assert_cmpint (xd_storage_get_last_seen (fixture->storage, chat_id, "claude"),
                   ==, after_claude);
  unseen = xd_storage_list_messages_since (fixture->storage, chat_id,
                                           after_claude, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (unseen->len, ==, 2);
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (unseen, 1))->content, ==,
                   "Codex here");

  /* Codex, which has answered but never been given a starting point, is owed
   * the whole conversation. */
  g_assert_cmpint (xd_storage_get_last_seen (fixture->storage, chat_id, "codex"), ==, 0);
}

/* A session that cannot be resumed remembers nothing, so forgetting it must
 * also reset what that backend is assumed to have seen. */
static void
test_forgetting_a_session_replays_everything (Fixture       *fixture,
                                              gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  xd_storage_append_message (fixture->storage, chat_id, "user", "hello", NULL, NULL, &error);
  xd_storage_set_session_id (fixture->storage, chat_id, "claude", "sess", &error);
  xd_storage_set_last_seen (fixture->storage, chat_id, "claude",
                            xd_storage_last_message_id (fixture->storage, chat_id),
                            &error);
  g_assert_no_error (error);
  g_assert_cmpint (xd_storage_get_last_seen (fixture->storage, chat_id, "claude"), >, 0);

  g_assert_true (xd_storage_set_session_id (fixture->storage, chat_id, "claude",
                                            NULL, &error));
  g_assert_cmpint (xd_storage_get_last_seen (fixture->storage, chat_id, "claude"), ==, 0);
}

static void
test_context_usage_follows_session (Fixture       *fixture,
                                    gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  guint64 used = 0;
  guint64 window = 0;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", "claude-opus-5", NULL, NULL,
                                    &error);
  g_assert_true (xd_storage_set_session_id (
    fixture->storage, chat_id, "claude", "sess", &error));
  g_assert_true (xd_storage_set_context_usage (
    fixture->storage, chat_id, "claude", "claude-opus-5",
    48750, 1000000, &error));
  g_assert_no_error (error);

  g_assert_true (xd_storage_get_context_usage (
    fixture->storage, chat_id, "claude", "claude-opus-5",
    &used, &window));
  g_assert_cmpuint (used, ==, 48750);
  g_assert_cmpuint (window, ==, 1000000);

  /* Changing model must not present the old model's usage as current. */
  g_assert_false (xd_storage_get_context_usage (
    fixture->storage, chat_id, "claude", "claude-haiku-4-5",
    &used, &window));

  /* A session that cannot resume has no live context either. */
  g_assert_true (xd_storage_set_session_id (
    fixture->storage, chat_id, "claude", NULL, &error));
  g_assert_false (xd_storage_get_context_usage (
    fixture->storage, chat_id, "claude", "claude-opus-5",
    &used, &window));
}

/*
 * Plan sits alongside the access level rather than replacing it, so leaving
 * plan puts the chat back where it was instead of dropping it to read-only.
 */
static void
test_plan_preserves_the_access_level (Fixture       *fixture,
                                      gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (XdChat) planning = NULL;
  g_autoptr (XdChat) building = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_true (xd_storage_set_access (fixture->storage, chat_id, "full", &error));
  g_assert_true (xd_storage_set_plan (fixture->storage, chat_id, TRUE, &error));
  g_assert_no_error (error);

  planning = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_true (planning->plan);
  g_assert_cmpstr (planning->access, ==, "full");

  g_assert_true (xd_storage_set_plan (fixture->storage, chat_id, FALSE, &error));
  building = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_false (building->plan);
  g_assert_cmpstr (building->access, ==, "full");
}

static void
test_workspace_locks_after_first_message (Fixture       *fixture,
                                          gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (XdChat) chat = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_true (xd_storage_set_new_worktree (
    fixture->storage, chat_id, TRUE, &error));
  g_assert_no_error (error);

  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_true (chat->new_worktree);

  g_assert_true (xd_storage_use_existing_worktree (
    fixture->storage, chat_id, "/tmp/existing-worktree",
    "/tmp/original-checkout", &error));
  g_assert_no_error (error);
  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_false (chat->new_worktree);
  g_assert_cmpstr (chat->workdir, ==, "/tmp/existing-worktree");
  g_assert_cmpstr (chat->original_workdir, ==, "/tmp/original-checkout");

  g_assert_true (xd_storage_append_message (
    fixture->storage, chat_id, "user", "start", NULL, NULL, &error));
  g_assert_no_error (error);

  g_assert_false (xd_storage_use_existing_worktree (
    fixture->storage, chat_id, "/tmp/another-worktree",
    "/tmp/original-checkout", &error));
  g_assert_error (error, G_IO_ERROR, G_IO_ERROR_FAILED);
}

static void
test_workspace_restores_its_first_checkout (Fixture       *fixture,
                                            gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (XdChat) chat = NULL;

  chat_id = xd_storage_create_chat (
    fixture->storage, "folder", "Chat", "claude",
    NULL, NULL, "/tmp/original-checkout", &error);
  g_assert_true (xd_storage_switch_workdir (
    fixture->storage, chat_id, "/tmp/first-worktree",
    "/tmp/original-checkout", &error));
  g_assert_true (xd_storage_switch_workdir (
    fixture->storage, chat_id, "/tmp/second-worktree",
    "/tmp/first-worktree", &error));
  g_assert_no_error (error);

  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (chat->workdir, ==, "/tmp/second-worktree");
  g_assert_cmpstr (chat->original_workdir, ==, "/tmp/original-checkout");

  g_assert_true (xd_storage_restore_workdir (
    fixture->storage, chat_id, chat->original_workdir, &error));
  g_assert_no_error (error);
  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_cmpstr (chat->workdir, ==, "/tmp/original-checkout");
  g_assert_null (chat->original_workdir);
}

/* Re-reporting overwrites rather than accumulating: the CLI hands back an id
 * on every turn, and only the latest one resumes. */
static void
test_session_id_is_replaced (Fixture       *fixture,
                             gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *session = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  xd_storage_set_session_id (fixture->storage, chat_id, "claude", "first", &error);
  xd_storage_set_session_id (fixture->storage, chat_id, "claude", "second", &error);
  g_assert_no_error (error);

  session = xd_storage_get_session_id (fixture->storage, chat_id, "claude", &error);
  g_assert_cmpstr (session, ==, "second");
}

static void
test_deleting_a_chat_takes_its_messages (Fixture       *fixture,
                                         gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (xd_storage_append_message (fixture->storage, chat_id, "user",
                                            "hello", NULL, NULL, &error));
  g_assert_true (xd_storage_delete_chat (fixture->storage, chat_id, &error));
  g_assert_no_error (error);

  messages = xd_storage_list_messages (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 0);
}

static void
test_search_finds_messages (Fixture       *fixture,
                            gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) hits = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  xd_storage_append_message (fixture->storage, chat_id, "user",
                             "the websocket reconnect loop is wrong", NULL, NULL, &error);
  xd_storage_append_message (fixture->storage, chat_id, "assistant",
                             "add exponential backoff", NULL, NULL, &error);
  g_assert_no_error (error);

  hits = xd_storage_search (fixture->storage, "websocket", 10, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (hits->len, ==, 1);
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (hits, 0))->content, ==,
                   "the websocket reconnect loop is wrong");

  /* The FTS index must forget deleted rows, or search returns dangling hits. */
  g_clear_pointer (&hits, g_ptr_array_unref);
  g_assert_true (xd_storage_delete_chat (fixture->storage, chat_id, &error));

  hits = xd_storage_search (fixture->storage, "websocket", 10, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (hits->len, ==, 0);
}

static void
test_reopening_keeps_data (Fixture       *fixture,
                           gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (XdStorage) reopened = NULL;
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *db_path = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  xd_storage_append_message (fixture->storage, chat_id, "user", "persist me",
                             NULL, NULL, &error);
  xd_storage_queue_append (fixture->storage, chat_id, "send me next", &error);
  g_assert_no_error (error);

  g_clear_object (&fixture->storage);

  db_path = g_build_filename (fixture->dir, "chats.db", NULL);
  reopened = xd_storage_new (db_path, &error);
  g_assert_no_error (error);

  messages = xd_storage_list_messages (reopened, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 1);
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 0))->content, ==,
                   "persist me");

  chat = xd_storage_get_chat (reopened, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 1);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==, "send me next");

  g_assert_true (xd_storage_set_queue (reopened, chat_id, NULL, &error));
  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (reopened, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 0);

  fixture->storage = g_object_ref (reopened);
}

static void
test_restart_markers_preserve_the_queue (Fixture       *fixture,
                                         gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) marked = g_ptr_array_new ();
  g_autoptr (GPtrArray) resumed = NULL;
  g_autoptr (GPtrArray) empty = NULL;
  g_autoptr (XdChat) chat = NULL;
  g_autofree char *first = NULL;
  g_autofree char *second = NULL;

  first = xd_storage_create_chat (fixture->storage, "folder", "First",
                                  "claude", NULL, NULL, NULL, &error);
  second = xd_storage_create_chat (fixture->storage, "folder", "Second",
                                   "codex", NULL, NULL, NULL, &error);
  g_assert_no_error (error);
  g_assert_true (xd_storage_queue_append (
    fixture->storage, first, "user queued this", &error));

  g_ptr_array_add (marked, first);
  g_ptr_array_add (marked, second);
  g_assert_true (xd_storage_mark_resumes (
    fixture->storage, marked, &error));
  g_assert_no_error (error);

  resumed = xd_storage_take_resumes (fixture->storage, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (resumed->len, ==, 2);
  g_assert_true (
    (g_strcmp0 (g_ptr_array_index (resumed, 0), first) == 0 &&
     g_strcmp0 (g_ptr_array_index (resumed, 1), second) == 0) ||
    (g_strcmp0 (g_ptr_array_index (resumed, 0), second) == 0 &&
     g_strcmp0 (g_ptr_array_index (resumed, 1), first) == 0));

  chat = xd_storage_get_chat (fixture->storage, first, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 1);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==, "user queued this");

  empty = xd_storage_take_resumes (fixture->storage, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (empty->len, ==, 0);
}

/*
 * More than one message can wait, and they are answered in the order written.
 */
static void
test_queue_keeps_every_message (Fixture       *fixture,
                                gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (XdChat) chat = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *first = NULL;
  g_autofree char *second = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (xd_storage_queue_append (
    fixture->storage, chat_id, "first thing", &error));
  g_assert_true (xd_storage_queue_append (
    fixture->storage, chat_id, "second thing", &error));
  g_assert_true (xd_storage_queue_append (
    fixture->storage, chat_id, "third thing", &error));

  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 3);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==, "first thing");
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 2), ==, "third thing");

  /* Steering one makes it next without disturbing the others. */
  g_assert_true (xd_storage_queue_promote (
    fixture->storage, chat_id, 2, &error));
  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==, "third thing");
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 1), ==, "first thing");
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 2), ==, "second thing");

  /* Editing changes only the selected message. */
  g_assert_true (xd_storage_queue_replace (
    fixture->storage, chat_id, 1, "first thing", "edited first thing", &error));
  g_assert_false (xd_storage_queue_replace (
    fixture->storage, chat_id, 1, "first thing", "stale edit", &error));
  g_assert_error (error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT);
  g_clear_error (&error);
  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==, "third thing");
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 1), ==,
                   "edited first thing");
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 2), ==, "second thing");

  /* Dropping one leaves the rest in their promoted order. */
  g_assert_true (xd_storage_queue_remove (fixture->storage, chat_id, 2, &error));

  /* Oldest first, and consumed as it is taken. */
  g_assert_true (xd_storage_queue_take_first (
    fixture->storage, chat_id, &first, &error));
  g_assert_cmpstr (first, ==, "third thing");
  g_assert_true (xd_storage_queue_take_first (
    fixture->storage, chat_id, &second, &error));
  g_assert_cmpstr (second, ==, "edited first thing");

  g_clear_pointer (&chat, xd_chat_free);
  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 0);

  /* Nothing waiting is not a failure; it just takes nothing. */
  {
    g_autofree char *nothing = NULL;

    g_assert_true (xd_storage_queue_take_first (
      fixture->storage, chat_id, &nothing, &error));
    g_assert_null (nothing);
  }
}

/*
 * A queue written before it could hold more than one message is a plain string
 * in that column. It has to read back as the one message it is, or an
 * instruction typed just before an update would be lost.
 */
static void
test_queue_reads_a_pre_list_row (Fixture       *fixture,
                                 gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (XdChat) chat = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *db_path = NULL;
  g_autofree char *sql = NULL;
  sqlite3 *db = NULL;

  chat_id = xd_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  /* What the old code stored: the message itself, not a list of them. */
  db_path = g_build_filename (fixture->dir, "chats.db", NULL);
  sql = g_strdup_printf (
    "UPDATE chats SET queued = 'typed before the update' WHERE id = '%s';",
    chat_id);
  g_assert_cmpint (sqlite3_open (db_path, &db), ==, SQLITE_OK);
  g_assert_cmpint (sqlite3_exec (db, sql, NULL, NULL, NULL), ==, SQLITE_OK);
  sqlite3_close (db);

  chat = xd_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chat->queue->len, ==, 1);
  g_assert_cmpstr (g_ptr_array_index (chat->queue, 0), ==,
                   "typed before the update");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

#define ADD(path, func) \
  g_test_add (path, Fixture, NULL, fixture_set_up, func, fixture_tear_down)

  ADD ("/storage/create-and-list", test_create_and_list);
  ADD ("/storage/chats-follow-latest-user-message",
       test_chats_follow_latest_user_message);
  ADD ("/storage/new-chats-inherit-agent", test_new_chats_inherit_last_changed_agent);
  ADD ("/storage/chats-follow-folder-id", test_chats_follow_folder_id);
  ADD ("/storage/messages-round-trip", test_messages_round_trip);
  ADD ("/storage/live-message-removed", test_live_message_can_be_removed);
  ADD ("/storage/recent-messages-bounded", test_recent_messages_are_bounded);
  ADD ("/storage/sessions-per-backend", test_sessions_are_per_backend);
  ADD ("/storage/forget-one-session", test_forgetting_one_session);
  ADD ("/storage/session-id-replaced", test_session_id_is_replaced);
  ADD ("/storage/tracks-what-was-seen", test_each_backend_tracks_what_it_has_seen);
  ADD ("/storage/forgetting-replays", test_forgetting_a_session_replays_everything);
  ADD ("/storage/context-usage", test_context_usage_follows_session);
  ADD ("/storage/plan-keeps-access", test_plan_preserves_the_access_level);
  ADD ("/storage/workspace-locks", test_workspace_locks_after_first_message);
  ADD ("/storage/workspace-restores-first-checkout", test_workspace_restores_its_first_checkout);
  ADD ("/storage/delete-cascades", test_deleting_a_chat_takes_its_messages);
  ADD ("/storage/search", test_search_finds_messages);
  ADD ("/storage/reopen", test_reopening_keeps_data);
  ADD ("/storage/restart-markers-keep-queue", test_restart_markers_preserve_the_queue);
  ADD ("/storage/queue-keeps-every-message", test_queue_keeps_every_message);
  ADD ("/storage/queue-reads-pre-list-row", test_queue_reads_a_pre_list_row);

#undef ADD

  return g_test_run ();
}
