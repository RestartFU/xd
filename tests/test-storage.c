#include <glib/gstdio.h>

#include "storage/storage.h"

typedef struct
{
  char *dir;
  HyStorage *storage;
} Fixture;

static void
fixture_set_up (Fixture       *fixture,
                gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *db_path = NULL;

  fixture->dir = g_dir_make_tmp ("hy-storage-XXXXXX", &error);
  g_assert_no_error (error);

  db_path = g_build_filename (fixture->dir, "chats.db", NULL);
  fixture->storage = hy_storage_new (db_path, &error);
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
  const HyChat *chat;

  first = hy_storage_create_chat (fixture->storage, "folder-a", "Rate limiting",
                                  "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);
  g_assert_nonnull (first);

  second = hy_storage_create_chat (fixture->storage, "folder-b", "Elsewhere",
                                   "codex", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  chats = hy_storage_list_chats (fixture->storage, "folder-a", &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chats->len, ==, 1);

  chat = g_ptr_array_index (chats, 0);
  g_assert_cmpstr (chat->id, ==, first);
  g_assert_cmpstr (chat->title, ==, "Rate limiting");
  g_assert_cmpstr (chat->backend, ==, "claude");
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

  chat_id = hy_storage_create_chat (fixture->storage, "stable-uuid", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  chats = hy_storage_list_chats (fixture->storage, "stable-uuid", &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chats->len, ==, 1);
  g_assert_cmpstr (((HyChat *) g_ptr_array_index (chats, 0))->id, ==, chat_id);
}

static void
test_messages_round_trip (Fixture       *fixture,
                          gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (hy_storage_append_message (fixture->storage, chat_id, "user",
                                            "how do I add a rate limiter?",
                                            NULL, &error));
  g_assert_no_error (error);

  g_assert_true (hy_storage_append_message (fixture->storage, chat_id, "assistant",
                                            "Use a token bucket.",
                                            "{\"type\":\"result\"}", &error));
  g_assert_no_error (error);

  messages = hy_storage_list_messages (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 2);

  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (messages, 0))->role, ==, "user");
  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (messages, 1))->content, ==,
                   "Use a token bucket.");
  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (messages, 1))->raw_json, ==,
                   "{\"type\":\"result\"}");
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

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (hy_storage_set_session_id (fixture->storage, chat_id,
                                            "claude", "sess-claude", &error));
  g_assert_true (hy_storage_set_session_id (fixture->storage, chat_id,
                                            "codex", "sess-codex", &error));
  g_assert_no_error (error);

  claude_session = hy_storage_get_session_id (fixture->storage, chat_id,
                                              "claude", &error);
  codex_session = hy_storage_get_session_id (fixture->storage, chat_id,
                                             "codex", &error);
  g_assert_no_error (error);
  g_assert_cmpstr (claude_session, ==, "sess-claude");
  g_assert_cmpstr (codex_session, ==, "sess-codex");

  /* A backend the chat has never used has nothing to resume. */
  missing = hy_storage_get_session_id (fixture->storage, chat_id, "nobody", &error);
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

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  hy_storage_set_session_id (fixture->storage, chat_id, "claude", "sess-a", &error);
  hy_storage_set_session_id (fixture->storage, chat_id, "codex", "sess-b", &error);
  g_assert_no_error (error);

  g_assert_true (hy_storage_set_session_id (fixture->storage, chat_id,
                                            "claude", NULL, &error));
  g_assert_no_error (error);

  claude_session = hy_storage_get_session_id (fixture->storage, chat_id,
                                              "claude", &error);
  codex_session = hy_storage_get_session_id (fixture->storage, chat_id,
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

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);

  /* Claude answers the first exchange. */
  hy_storage_append_message (fixture->storage, chat_id, "user", "who are you", NULL, &error);
  hy_storage_append_message (fixture->storage, chat_id, "assistant", "Claude here", NULL, &error);
  g_assert_no_error (error);

  after_claude = hy_storage_last_message_id (fixture->storage, chat_id);
  g_assert_true (hy_storage_set_last_seen (fixture->storage, chat_id, "claude",
                                           after_claude, &error));

  /* Then the user switches to Codex for a turn. */
  hy_storage_append_message (fixture->storage, chat_id, "user", "and you?", NULL, &error);
  hy_storage_append_message (fixture->storage, chat_id, "assistant", "Codex here", NULL, &error);
  g_assert_no_error (error);

  /* Claude has never been told about that exchange. */
  g_assert_cmpint (hy_storage_get_last_seen (fixture->storage, chat_id, "claude"),
                   ==, after_claude);
  unseen = hy_storage_list_messages_since (fixture->storage, chat_id,
                                           after_claude, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (unseen->len, ==, 2);
  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (unseen, 1))->content, ==,
                   "Codex here");

  /* Codex, which has answered but never been given a starting point, is owed
   * the whole conversation. */
  g_assert_cmpint (hy_storage_get_last_seen (fixture->storage, chat_id, "codex"), ==, 0);
}

/* A session that cannot be resumed remembers nothing, so forgetting it must
 * also reset what that backend is assumed to have seen. */
static void
test_forgetting_a_session_replays_everything (Fixture       *fixture,
                                              gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  hy_storage_append_message (fixture->storage, chat_id, "user", "hello", NULL, &error);
  hy_storage_set_session_id (fixture->storage, chat_id, "claude", "sess", &error);
  hy_storage_set_last_seen (fixture->storage, chat_id, "claude",
                            hy_storage_last_message_id (fixture->storage, chat_id),
                            &error);
  g_assert_no_error (error);
  g_assert_cmpint (hy_storage_get_last_seen (fixture->storage, chat_id, "claude"), >, 0);

  g_assert_true (hy_storage_set_session_id (fixture->storage, chat_id, "claude",
                                            NULL, &error));
  g_assert_cmpint (hy_storage_get_last_seen (fixture->storage, chat_id, "claude"), ==, 0);
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
  g_autoptr (HyChat) planning = NULL;
  g_autoptr (HyChat) building = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_true (hy_storage_set_access (fixture->storage, chat_id, "full", &error));
  g_assert_true (hy_storage_set_plan (fixture->storage, chat_id, TRUE, &error));
  g_assert_no_error (error);

  planning = hy_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_true (planning->plan);
  g_assert_cmpstr (planning->access, ==, "full");

  g_assert_true (hy_storage_set_plan (fixture->storage, chat_id, FALSE, &error));
  building = hy_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_false (building->plan);
  g_assert_cmpstr (building->access, ==, "full");
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

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  hy_storage_set_session_id (fixture->storage, chat_id, "claude", "first", &error);
  hy_storage_set_session_id (fixture->storage, chat_id, "claude", "second", &error);
  g_assert_no_error (error);

  session = hy_storage_get_session_id (fixture->storage, chat_id, "claude", &error);
  g_assert_cmpstr (session, ==, "second");
}

static void
test_deleting_a_chat_takes_its_messages (Fixture       *fixture,
                                         gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  g_assert_true (hy_storage_append_message (fixture->storage, chat_id, "user",
                                            "hello", NULL, &error));
  g_assert_true (hy_storage_delete_chat (fixture->storage, chat_id, &error));
  g_assert_no_error (error);

  messages = hy_storage_list_messages (fixture->storage, chat_id, &error);
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

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  hy_storage_append_message (fixture->storage, chat_id, "user",
                             "the websocket reconnect loop is wrong", NULL, &error);
  hy_storage_append_message (fixture->storage, chat_id, "assistant",
                             "add exponential backoff", NULL, &error);
  g_assert_no_error (error);

  hits = hy_storage_search (fixture->storage, "websocket", 10, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (hits->len, ==, 1);
  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (hits, 0))->content, ==,
                   "the websocket reconnect loop is wrong");

  /* The FTS index must forget deleted rows, or search returns dangling hits. */
  g_clear_pointer (&hits, g_ptr_array_unref);
  g_assert_true (hy_storage_delete_chat (fixture->storage, chat_id, &error));

  hits = hy_storage_search (fixture->storage, "websocket", 10, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (hits->len, ==, 0);
}

static void
test_reopening_keeps_data (Fixture       *fixture,
                           gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (HyStorage) reopened = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *db_path = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", NULL, NULL, NULL, &error);
  hy_storage_append_message (fixture->storage, chat_id, "user", "persist me",
                             NULL, &error);
  g_assert_no_error (error);

  g_clear_object (&fixture->storage);

  db_path = g_build_filename (fixture->dir, "chats.db", NULL);
  reopened = hy_storage_new (db_path, &error);
  g_assert_no_error (error);

  messages = hy_storage_list_messages (reopened, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (messages->len, ==, 1);
  g_assert_cmpstr (((HyMessage *) g_ptr_array_index (messages, 0))->content, ==,
                   "persist me");

  fixture->storage = g_object_ref (reopened);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

#define ADD(path, func) \
  g_test_add (path, Fixture, NULL, fixture_set_up, func, fixture_tear_down)

  ADD ("/storage/create-and-list", test_create_and_list);
  ADD ("/storage/chats-follow-folder-id", test_chats_follow_folder_id);
  ADD ("/storage/messages-round-trip", test_messages_round_trip);
  ADD ("/storage/sessions-per-backend", test_sessions_are_per_backend);
  ADD ("/storage/forget-one-session", test_forgetting_one_session);
  ADD ("/storage/session-id-replaced", test_session_id_is_replaced);
  ADD ("/storage/tracks-what-was-seen", test_each_backend_tracks_what_it_has_seen);
  ADD ("/storage/forgetting-replays", test_forgetting_a_session_replays_everything);
  ADD ("/storage/plan-keeps-access", test_plan_preserves_the_access_level);
  ADD ("/storage/delete-cascades", test_deleting_a_chat_takes_its_messages);
  ADD ("/storage/search", test_search_finds_messages);
  ADD ("/storage/reopen", test_reopening_keeps_data);

#undef ADD

  return g_test_run ();
}
