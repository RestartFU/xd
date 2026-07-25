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
                                  "claude", &error);
  g_assert_no_error (error);
  g_assert_nonnull (first);

  second = hy_storage_create_chat (fixture->storage, "folder-b", "Elsewhere",
                                   "codex", &error);
  g_assert_no_error (error);

  chats = hy_storage_list_chats (fixture->storage, "folder-a", &error);
  g_assert_no_error (error);
  g_assert_cmpuint (chats->len, ==, 1);

  chat = g_ptr_array_index (chats, 0);
  g_assert_cmpstr (chat->id, ==, first);
  g_assert_cmpstr (chat->title, ==, "Rate limiting");
  g_assert_cmpstr (chat->backend, ==, "claude");
  g_assert_null (chat->session_id);
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
                                    "claude", &error);
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
                                    "claude", &error);
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

static void
test_session_id_persists (Fixture       *fixture,
                          gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (HyChat) chat = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", &error);
  g_assert_no_error (error);

  g_assert_true (hy_storage_set_session_id (fixture->storage, chat_id,
                                            "sess-123", &error));
  g_assert_no_error (error);

  chat = hy_storage_get_chat (fixture->storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_cmpstr (chat->session_id, ==, "sess-123");
}

static void
test_deleting_a_chat_takes_its_messages (Fixture       *fixture,
                                         gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *chat_id = NULL;

  chat_id = hy_storage_create_chat (fixture->storage, "folder", "Chat",
                                    "claude", &error);
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
                                    "claude", &error);
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
                                    "claude", &error);
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
  ADD ("/storage/session-id-persists", test_session_id_persists);
  ADD ("/storage/delete-cascades", test_deleting_a_chat_takes_its_messages);
  ADD ("/storage/search", test_search_finds_messages);
  ADD ("/storage/reopen", test_reopening_keeps_data);

#undef ADD

  return g_test_run ();
}
